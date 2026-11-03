//! Native AIFF metadata chunks.
//!
//! TagLib exposes ID3v2 in AIFF, but many tools (including ffmpeg without
//! `-write_id3v2 1`) use the format's own `NAME`, `AUTH`, `(c) `, and `ANNO`
//! chunks instead. This module reads those chunks and rewrites them while
//! preserving every other chunk, including the audio data and any ID3 tag.

use anyhow::{anyhow, Result};
use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::io::{Read, Seek, SeekFrom};
use std::path::Path;

#[derive(Clone, Debug)]
struct Chunk {
    id: [u8; 4],
    data: Vec<u8>,
}

pub fn read_native(path: &Path) -> Result<BTreeMap<String, Vec<String>>> {
    let mut file = fs::File::open(path)?;
    let length = file.metadata()?.len();
    let mut header = [0u8; 12];
    if length < 12 {
        return Err(anyhow!("not an AIFF file"));
    }
    file.read_exact(&mut header)?;
    if &header[..4] != b"FORM" {
        return Err(anyhow!("not an AIFF file"));
    }
    let form_type = [header[8], header[9], header[10], header[11]];
    if &form_type != b"AIFF" && &form_type != b"AIFC" {
        return Err(anyhow!("unsupported IFF form"));
    }

    let mut fields: BTreeMap<String, Vec<String>> = BTreeMap::new();
    let mut position = 12u64;
    while position + 8 <= length {
        file.seek(SeekFrom::Start(position))?;
        let mut chunk_header = [0u8; 8];
        file.read_exact(&mut chunk_header)?;
        let id = [
            chunk_header[0],
            chunk_header[1],
            chunk_header[2],
            chunk_header[3],
        ];
        let size = u32::from_be_bytes([
            chunk_header[4],
            chunk_header[5],
            chunk_header[6],
            chunk_header[7],
        ]) as u64;
        let start = position + 8;
        let end = start
            .checked_add(size)
            .ok_or_else(|| anyhow!("invalid AIFF chunk size"))?;
        if end > length {
            return Err(anyhow!("truncated AIFF chunk"));
        }
        if let Some(field) = field_for_chunk(&id) {
            if size > 32 * 1024 * 1024 {
                return Err(anyhow!("AIFF metadata chunk is unreasonably large"));
            }
            let mut data = vec![0u8; size as usize];
            file.read_exact(&mut data)?;
            let value = decode_text(&data);
            if !value.is_empty() {
                fields.entry(field.to_string()).or_default().push(value);
            }
        }
        position = end + (size % 2);
    }
    Ok(fields)
}

pub fn write_native(path: &Path, fields: &BTreeMap<String, Vec<String>>) -> Result<()> {
    let bytes = fs::read(path)?;
    let (form_type, chunks) = parse_chunks(&bytes)?;

    let mut output_chunks = Vec::with_capacity(chunks.len() + 4);
    let mut seen = BTreeSet::new();
    for chunk in chunks {
        let Some(field) = field_for_chunk(&chunk.id) else {
            output_chunks.push(chunk);
            continue;
        };
        if !seen.insert(field.to_string()) {
            continue;
        }
        let values = fields
            .get(field)
            .map(|values| {
                values
                    .iter()
                    .filter(|value| !value.is_empty())
                    .cloned()
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();
        for value in values {
            output_chunks.push(Chunk {
                id: chunk.id,
                data: value.into_bytes(),
            });
        }
    }

    // New native fields go immediately before the audio data, where ffmpeg
    // and other writers place them, rather than after a potentially huge SSND.
    let insert_at = output_chunks
        .iter()
        .position(|chunk| &chunk.id == b"SSND")
        .unwrap_or(output_chunks.len());
    let mut additions = Vec::new();
    for id in [*b"NAME", *b"AUTH", *b"(c) ", *b"ANNO"] {
        let field = field_for_chunk(&id).unwrap();
        if seen.contains(field) {
            continue;
        }
        if let Some(values) = fields.get(field) {
            for value in values.iter().filter(|value| !value.is_empty()) {
                additions.push(Chunk {
                    id,
                    data: value.as_bytes().to_vec(),
                });
            }
        }
    }
    output_chunks.splice(insert_at..insert_at, additions);

    let mut body = Vec::new();
    body.extend_from_slice(&form_type);
    for chunk in output_chunks {
        body.extend_from_slice(&chunk.id);
        body.extend_from_slice(&(chunk.data.len() as u32).to_be_bytes());
        body.extend_from_slice(&chunk.data);
        if chunk.data.len() % 2 == 1 {
            body.push(0);
        }
    }
    if body.len() > u32::MAX as usize {
        return Err(anyhow!("AIFF file is too large to rewrite"));
    }
    let mut output = Vec::with_capacity(body.len() + 8);
    output.extend_from_slice(b"FORM");
    output.extend_from_slice(&(body.len() as u32).to_be_bytes());
    output.extend_from_slice(&body);

    let temporary = path.with_extension(format!(
        "{}.puddlemore.tmp",
        path.extension()
            .and_then(|ext| ext.to_str())
            .unwrap_or_default()
    ));
    fs::write(&temporary, output)?;
    let permissions = fs::metadata(path)?.permissions();
    fs::set_permissions(&temporary, permissions)?;
    fs::rename(&temporary, path)?;
    Ok(())
}

fn parse_chunks(bytes: &[u8]) -> Result<([u8; 4], Vec<Chunk>)> {
    if bytes.len() < 12 || &bytes[..4] != b"FORM" {
        return Err(anyhow!("not an AIFF file"));
    }
    let form_type = [bytes[8], bytes[9], bytes[10], bytes[11]];
    if &form_type != b"AIFF" && &form_type != b"AIFC" {
        return Err(anyhow!("unsupported IFF form"));
    }

    let mut chunks = Vec::new();
    let mut position = 12usize;
    while position + 8 <= bytes.len() {
        let id = [
            bytes[position],
            bytes[position + 1],
            bytes[position + 2],
            bytes[position + 3],
        ];
        let size = u32::from_be_bytes([
            bytes[position + 4],
            bytes[position + 5],
            bytes[position + 6],
            bytes[position + 7],
        ]) as usize;
        let start = position + 8;
        let end = start
            .checked_add(size)
            .ok_or_else(|| anyhow!("invalid AIFF chunk size"))?;
        if end > bytes.len() {
            return Err(anyhow!("truncated AIFF chunk"));
        }
        chunks.push(Chunk {
            id,
            data: bytes[start..end].to_vec(),
        });
        position = end + (size % 2);
    }
    Ok((form_type, chunks))
}

fn field_for_chunk(id: &[u8; 4]) -> Option<&'static str> {
    match id {
        b"NAME" => Some("title"),
        b"AUTH" => Some("artist"),
        b"(c) " => Some("copyright"),
        b"ANNO" => Some("comment"),
        _ => None,
    }
}

fn decode_text(data: &[u8]) -> String {
    let trimmed = data.strip_suffix(&[0]).unwrap_or(data);
    String::from_utf8_lossy(trimmed).trim_end().to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn maps_native_chunks() {
        assert_eq!(field_for_chunk(b"NAME"), Some("title"));
        assert_eq!(field_for_chunk(b"AUTH"), Some("artist"));
        assert_eq!(field_for_chunk(b"(c) "), Some("copyright"));
        assert_eq!(field_for_chunk(b"ANNO"), Some("comment"));
        assert_eq!(field_for_chunk(b"SSND"), None);
    }

    #[test]
    fn decodes_padded_text() {
        assert_eq!(decode_text(b"hello\0"), "hello");
        assert_eq!(decode_text(b"hello"), "hello");
    }
}
