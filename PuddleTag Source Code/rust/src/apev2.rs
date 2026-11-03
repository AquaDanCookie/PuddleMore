//! Minimal APEv2 tag reader/writer for containers TagLib only exposes ID3 for.
//!
//! TrueAudio files created by ffmpeg (and many other tools) carry APEv2 tags
//! rather than ID3v2. TagLib's TrueAudio reader only looks at ID3, so without
//! this module those tags are invisible and an edit would create a second,
//! conflicting tag. APEv2 is a deliberately small format: a sequence of
//! key/value items followed by a fixed 32-byte footer (and usually a header).

use anyhow::{anyhow, Result};
use std::collections::{BTreeMap, HashMap};
use std::fs;
use std::io::{Read, Seek, SeekFrom};
use std::path::Path;

const SIGNATURE: &[u8; 8] = b"APETAGEX";
const FOOTER_SIZE: usize = 32;
const VERSION: u32 = 2000;
const FLAG_HAS_HEADER: u32 = 0x8000_0000;
const FLAG_IS_HEADER: u32 = 0x2000_0000;
const ITEM_BINARY: u32 = 0x2;

#[derive(Clone, Debug)]
struct BinaryItem {
    key: String,
    flags: u32,
    value: Vec<u8>,
}

#[derive(Clone, Debug)]
pub struct ApeTag {
    /// Canonical field name -> values.
    pub fields: BTreeMap<String, Vec<String>>,
    /// Canonical field name -> original APEv2 key.
    pub names: HashMap<String, String>,
    binary: Vec<BinaryItem>,
}

#[derive(Clone, Copy, Debug)]
struct Footer {
    position: u64,
    size: u32,
    count: u32,
    flags: u32,
}

impl Footer {
    fn has_header(&self) -> bool {
        self.flags & FLAG_HAS_HEADER != 0
    }

    /// Byte offset of the first item. `tag_size` includes the footer and the
    /// items but excludes an optional header.
    fn items_start(&self) -> u64 {
        self.position + FOOTER_SIZE as u64 - self.size as u64
    }

    /// Byte offset of the optional header.
    fn header_start(&self) -> u64 {
        self.items_start() - FOOTER_SIZE as u64
    }
}

/// Returns true when the file ends with an APEv2 tag (possibly followed by an
/// ID3v1 tag, which is preserved by the writer).
pub fn has_tag(path: &Path) -> bool {
    footer(path).is_some()
}

pub fn read(path: &Path) -> Result<ApeTag> {
    let mut file = fs::File::open(path)?;
    let footer = footer_from_file(&mut file)?
        .ok_or_else(|| anyhow!("no APEv2 tag found in {}", path.display()))?;
    let items_start = footer.items_start();
    let footer_position = footer.position;
    if items_start > footer_position {
        return Err(anyhow!("invalid APEv2 tag bounds"));
    }

    file.seek(SeekFrom::Start(items_start))?;
    let mut data = Vec::with_capacity((footer_position - items_start) as usize);
    file.take(footer_position - items_start)
        .read_to_end(&mut data)?;

    let mut fields: BTreeMap<String, Vec<String>> = BTreeMap::new();
    let mut names = HashMap::new();
    let mut binary = Vec::new();
    let mut offset = 0usize;
    for _ in 0..footer.count {
        if offset + 8 > data.len() {
            return Err(anyhow!("truncated APEv2 item header"));
        }
        let value_size = le_u32(&data[offset..offset + 4]) as usize;
        let flags = le_u32(&data[offset + 4..offset + 8]);
        offset += 8;
        let key_end = data[offset..]
            .iter()
            .position(|byte| *byte == 0)
            .ok_or_else(|| anyhow!("unterminated APEv2 item key"))?;
        let key = String::from_utf8_lossy(&data[offset..offset + key_end]).into_owned();
        offset += key_end + 1;
        if offset + value_size > data.len() {
            return Err(anyhow!("truncated APEv2 item value"));
        }
        let value = &data[offset..offset + value_size];
        offset += value_size;

        if flags & ITEM_BINARY != 0 {
            binary.push(BinaryItem {
                key,
                flags,
                value: value.to_vec(),
            });
            continue;
        }

        let values = value
            .split(|byte| *byte == 0)
            .filter(|part| !part.is_empty())
            .map(|part| String::from_utf8_lossy(part).into_owned())
            .collect::<Vec<_>>();
        if values.is_empty() {
            continue;
        }
        store_values(&mut fields, &mut names, &key, values);
    }

    Ok(ApeTag {
        fields,
        names,
        binary,
    })
}

pub fn remove_tag(path: &Path) -> Result<bool> {
    let bytes = fs::read(path)?;
    let Some(footer) = find_footer(&bytes) else {
        return Ok(false);
    };
    let start = if footer.has_header() {
        if footer.items_start() < FOOTER_SIZE as u64 {
            return Err(anyhow!("invalid APEv2 header bounds"));
        }
        footer.header_start()
    } else {
        footer.items_start()
    } as usize;
    let end = footer.position as usize + FOOTER_SIZE;
    if start > end || end > bytes.len() {
        return Err(anyhow!("invalid APEv2 tag bounds"));
    }
    let mut output = Vec::with_capacity(bytes.len() - (end - start));
    output.extend_from_slice(&bytes[..start]);
    output.extend_from_slice(&bytes[end..]);
    fs::write(path, output)?;
    Ok(true)
}

pub fn write(
    path: &Path,
    fields: &BTreeMap<String, Vec<String>>,
    names: &HashMap<String, String>,
) -> Result<()> {
    let bytes = fs::read(path)?;
    let old = find_footer(&bytes);
    let (prefix, suffix) = match old {
        Some(footer) => {
            let start = if footer.has_header() {
                if footer.items_start() < FOOTER_SIZE as u64 {
                    return Err(anyhow!("invalid APEv2 header bounds"));
                }
                let header = footer.header_start();
                if header > footer.position {
                    return Err(anyhow!("invalid APEv2 header bounds"));
                }
                header as usize
            } else {
                footer.items_start() as usize
            };
            let end = footer.position as usize + FOOTER_SIZE;
            if start > end || end > bytes.len() {
                return Err(anyhow!("invalid APEv2 tag bounds"));
            }
            // Keep tags such as ID3v1 that live after the APE footer.
            (bytes[..start].to_vec(), bytes[end..].to_vec())
        }
        None => (bytes, Vec::new()),
    };

    let binary = if old.is_some() {
        read(path)?.binary
    } else {
        Vec::new()
    };

    let mut items: Vec<(String, Vec<u8>, u32)> = Vec::new();
    for item in binary {
        items.push((item.key, item.value, item.flags));
    }
    for (field, values) in fields {
        if field.starts_with("__")
            || matches!(
                field.as_str(),
                "track" | "totaltracks" | "disc" | "totaldiscs"
            )
        {
            continue;
        }
        let values = values
            .iter()
            .filter(|value| !value.is_empty())
            .cloned()
            .collect::<Vec<_>>();
        if values.is_empty() {
            continue;
        }
        let key = names
            .get(field)
            .cloned()
            .unwrap_or_else(|| property_for_field(field));
        items.push((key, join_text_values(&values), 0));
    }
    for (number_field, total_field, fallback) in [
        ("track", "totaltracks", "Track"),
        ("disc", "totaldiscs", "Disc"),
    ] {
        let number = fields
            .get(number_field)
            .and_then(|values| values.iter().find(|value| !value.is_empty()));
        let total = fields
            .get(total_field)
            .and_then(|values| values.iter().find(|value| !value.is_empty()));
        if number.is_none() && total.is_none() {
            continue;
        }
        let key = names
            .get(number_field)
            .cloned()
            .unwrap_or_else(|| fallback.to_string());
        let value = match (number, total) {
            (Some(number), Some(total)) => format!("{number}/{total}"),
            (Some(number), None) => number.clone(),
            (None, Some(total)) => format!("0/{total}"),
            (None, None) => unreachable!(),
        };
        items.push((key, value.into_bytes(), 0));
    }

    let mut output = prefix;
    if !items.is_empty() {
        let mut item_data = Vec::new();
        for (key, value, flags) in &items {
            item_data.extend_from_slice(&(value.len() as u32).to_le_bytes());
            item_data.extend_from_slice(&flags.to_le_bytes());
            item_data.extend_from_slice(key.as_bytes());
            item_data.push(0);
            item_data.extend_from_slice(value);
        }
        let size = (item_data.len() + FOOTER_SIZE) as u32;
        let count = items.len() as u32;

        // Header: tag has a header and this record is the header.
        output.extend_from_slice(SIGNATURE);
        output.extend_from_slice(&VERSION.to_le_bytes());
        output.extend_from_slice(&size.to_le_bytes());
        output.extend_from_slice(&count.to_le_bytes());
        output.extend_from_slice(&(FLAG_HAS_HEADER | FLAG_IS_HEADER).to_le_bytes());
        output.extend_from_slice(&[0u8; 8]);
        output.extend_from_slice(&item_data);

        // Footer: tag has a header; this record is the footer.
        output.extend_from_slice(SIGNATURE);
        output.extend_from_slice(&VERSION.to_le_bytes());
        output.extend_from_slice(&size.to_le_bytes());
        output.extend_from_slice(&count.to_le_bytes());
        output.extend_from_slice(&FLAG_HAS_HEADER.to_le_bytes());
        output.extend_from_slice(&[0u8; 8]);
    }
    output.extend_from_slice(&suffix);

    // Write a sibling file first so an interrupted save cannot destroy the
    // original. TTA files can be large, so streaming would be nicer, but the
    // straightforward version is easier to audit.
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

fn store_values(
    fields: &mut BTreeMap<String, Vec<String>>,
    names: &mut HashMap<String, String>,
    key: &str,
    values: Vec<String>,
) {
    let field = canonical_field(key);
    names
        .entry(field.clone())
        .or_insert_with(|| key.to_string());
    if matches!(field.as_str(), "track" | "disc") && values.len() == 1 {
        if let Some((number, total)) = values[0].split_once('/') {
            let total_field = if field == "track" {
                "totaltracks"
            } else {
                "totaldiscs"
            };
            fields
                .entry(field.clone())
                .or_default()
                .push(number.trim().to_string());
            fields
                .entry(total_field.to_string())
                .or_default()
                .push(total.trim().to_string());
            names
                .entry(total_field.to_string())
                .or_insert_with(|| key.to_string());
            return;
        }
    }
    fields.entry(field).or_default().extend(values);
}

fn canonical_field(key: &str) -> String {
    match key.to_ascii_uppercase().as_str() {
        "TITLE" => "title",
        "ARTIST" => "artist",
        "ALBUM" => "album",
        "ALBUMARTIST" | "ALBUM ARTIST" => "albumartist",
        "COMPOSER" => "composer",
        "CONDUCTOR" => "conductor",
        "COMMENT" => "comment",
        "DESCRIPTION" => "description",
        "GENRE" => "genre",
        "YEAR" | "DATE" => "year",
        "TRACK" | "TRACKNUMBER" => "track",
        "TRACKTOTAL" | "TOTALTRACKS" => "totaltracks",
        "DISC" | "DISCNUMBER" => "disc",
        "DISCTOTAL" | "TOTALDISCS" => "totaldiscs",
        "BPM" => "bpm",
        "ISRC" => "isrc",
        "LANGUAGE" => "language",
        "LYRICS" | "UNSYNCEDLYRICS" => "lyrics",
        "COPYRIGHT" => "copyright",
        "ENCODEDBY" => "encodedby",
        "ENCODING" | "ENCODERSETTINGS" => "encodersettings",
        "ENCODER" => "encoder",
        "PUBLISHER" | "LABEL" => "organization",
        "CATALOGNUMBER" => "catalognumber",
        "MOOD" => "mood",
        "INITIALKEY" => "initialkey",
        "SUBTITLE" => "subtitle",
        "GROUPING" | "CONTENTGROUP" => "grouping",
        "WORK" => "work",
        "MOVEMENTNAME" => "movementname",
        "MOVEMENTNUMBER" => "movementnumber",
        "MOVEMENTTOTAL" => "movementtotal",
        "ORIGINALDATE" | "ORIGINALYEAR" => "originalyear",
        "ORIGINALALBUM" => "originalalbum",
        "ORIGINALARTIST" => "originalartist",
        "RELEASEDATE" => "releasedate",
        "LYRICIST" | "WRITER" => "lyricist",
        "ARRANGER" => "arranger",
        other => return other.to_ascii_lowercase(),
    }
    .to_string()
}

fn property_for_field(field: &str) -> String {
    match field.to_ascii_lowercase().as_str() {
        "title" => "Title",
        "artist" => "Artist",
        "album" => "Album",
        "albumartist" => "Album Artist",
        "composer" => "Composer",
        "conductor" => "Conductor",
        "comment" => "Comment",
        "description" => "Description",
        "genre" => "Genre",
        "year" => "Year",
        "bpm" => "BPM",
        "isrc" => "ISRC",
        "language" => "Language",
        "lyrics" => "Lyrics",
        "copyright" => "Copyright",
        "encodedby" => "EncodedBy",
        "encodersettings" => "EncoderSettings",
        "encoder" => "Encoder",
        "organization" => "Publisher",
        "catalognumber" => "CatalogNumber",
        "mood" => "Mood",
        "initialkey" => "InitialKey",
        "subtitle" => "Subtitle",
        "grouping" => "Grouping",
        "work" => "Work",
        "movementname" => "MovementName",
        "movementnumber" => "MovementNumber",
        "movementtotal" => "MovementTotal",
        "originalyear" => "OriginalYear",
        "originalalbum" => "OriginalAlbum",
        "originalartist" => "OriginalArtist",
        "releasedate" => "ReleaseDate",
        "lyricist" => "Lyricist",
        "arranger" => "Arranger",
        other => return other.to_ascii_uppercase(),
    }
    .to_string()
}

fn join_text_values(values: &[String]) -> Vec<u8> {
    let mut bytes = Vec::new();
    for (index, value) in values.iter().enumerate() {
        if index > 0 {
            bytes.push(0);
        }
        bytes.extend_from_slice(value.as_bytes());
    }
    bytes
}

fn footer(path: &Path) -> Option<Footer> {
    let mut file = fs::File::open(path).ok()?;
    footer_from_file(&mut file).ok().flatten()
}

fn footer_from_file(file: &mut fs::File) -> Result<Option<Footer>> {
    let length = file.metadata()?.len();
    if length < FOOTER_SIZE as u64 {
        return Ok(None);
    }
    let read_size = length.min(4096) as usize;
    file.seek(SeekFrom::Start(length - read_size as u64))?;
    let mut tail = vec![0u8; read_size];
    file.read_exact(&mut tail)?;
    let base = length - read_size as u64;
    Ok(find_footer_in(&tail, base))
}

fn find_footer(bytes: &[u8]) -> Option<Footer> {
    let start = bytes.len().saturating_sub(4096);
    find_footer_in(&bytes[start..], start as u64)
}

fn find_footer_in(bytes: &[u8], base: u64) -> Option<Footer> {
    let mut search_end = bytes.len();
    while let Some(index) = rfind_signature(&bytes[..search_end]) {
        let position = base + index as u64;
        if index + FOOTER_SIZE <= bytes.len() {
            if let Some(footer) = parse_footer(&bytes[index..index + FOOTER_SIZE], position) {
                if (footer.size as u64) <= position + FOOTER_SIZE as u64 {
                    let tag_start = position + FOOTER_SIZE as u64 - footer.size as u64;
                    if tag_start <= position {
                        return Some(footer);
                    }
                }
            }
        }
        if index == 0 {
            break;
        }
        search_end = index;
    }
    None
}

fn rfind_signature(bytes: &[u8]) -> Option<usize> {
    bytes
        .windows(SIGNATURE.len())
        .rposition(|window| window == SIGNATURE)
}

fn parse_footer(bytes: &[u8], position: u64) -> Option<Footer> {
    if bytes.len() < FOOTER_SIZE || &bytes[..8] != SIGNATURE {
        return None;
    }
    let version = le_u32(&bytes[8..12]);
    if version < 1000 || version > 2000 {
        return None;
    }
    let size = le_u32(&bytes[12..16]);
    let count = le_u32(&bytes[16..20]);
    let flags = le_u32(&bytes[20..24]);
    if (size as usize) < FOOTER_SIZE {
        return None;
    }
    Some(Footer {
        position,
        size,
        count,
        flags,
    })
}

fn le_u32(bytes: &[u8]) -> u32 {
    u32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn maps_ape_keys() {
        assert_eq!(canonical_field("Track"), "track");
        assert_eq!(canonical_field("Album Artist"), "albumartist");
        assert_eq!(canonical_field("CUSTOMFIELD"), "customfield");
        assert_eq!(property_for_field("customfield"), "CUSTOMFIELD");
    }

    #[test]
    fn accepts_only_plausible_footers() {
        let mut footer = [0u8; FOOTER_SIZE];
        footer[..8].copy_from_slice(SIGNATURE);
        footer[8..12].copy_from_slice(&VERSION.to_le_bytes());
        footer[12..16].copy_from_slice(&(FOOTER_SIZE as u32).to_le_bytes());
        assert!(parse_footer(&footer, 100).is_some());
        footer[12..16].copy_from_slice(&4u32.to_le_bytes());
        assert!(parse_footer(&footer, 100).is_none());
    }
}
