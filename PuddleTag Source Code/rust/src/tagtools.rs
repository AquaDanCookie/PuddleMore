//! Direct tag removal used by Tag Tools.
//!
//! ID3v1/ID3v2 are found by their fixed signatures; APEv2 removal reuses the
//! native APE reader's validated footer search.

use crate::apev2;
use anyhow::{Context, Result};
use std::fs;
use std::path::Path;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TagKind {
    Id3v1,
    Id3v2,
    Id3,
    Apev2,
}

pub fn remove(path: &Path, kind: TagKind) -> Result<bool> {
    match kind {
        TagKind::Id3v1 => remove_id3v1(path),
        TagKind::Id3v2 => remove_id3v2(path),
        TagKind::Id3 => {
            let a = remove_id3v2(path)?;
            let b = remove_id3v1(path)?;
            Ok(a || b)
        }
        TagKind::Apev2 => apev2::remove_tag(path),
    }
}

pub fn remove_id3v1(path: &Path) -> Result<bool> {
    let mut data = fs::read(path).with_context(|| format!("reading {}", path.display()))?;
    if data.len() >= 128 && &data[data.len() - 128..data.len() - 125] == b"TAG" {
        data.truncate(data.len() - 128);
        fs::write(path, data).with_context(|| format!("writing {}", path.display()))?;
        return Ok(true);
    }
    Ok(false)
}

pub fn remove_id3v2(path: &Path) -> Result<bool> {
    let mut data = fs::read(path).with_context(|| format!("reading {}", path.display()))?;
    if data.len() < 10 || &data[..3] != b"ID3" {
        return Ok(false);
    }
    let size = syncsafe(&data[6..10]);
    let mut end = 10usize.saturating_add(size);
    if data[5] & 0x10 != 0 {
        // ID3v2.4 footer.
        end = end.saturating_add(10);
    }
    if end > data.len() {
        return Ok(false);
    }
    data.drain(..end);
    fs::write(path, data).with_context(|| format!("writing {}", path.display()))?;
    Ok(true)
}

fn syncsafe(bytes: &[u8]) -> usize {
    ((bytes[0] as usize & 0x7f) << 21)
        | ((bytes[1] as usize & 0x7f) << 14)
        | ((bytes[2] as usize & 0x7f) << 7)
        | (bytes[3] as usize & 0x7f)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn removes_id3v2_and_id3v1() {
        let path =
            std::env::temp_dir().join(format!("puddlemore-tagtools-{}.mp3", std::process::id()));
        let mut data = b"ID3\x04\x00\x00\x00\x00\x00\x05hello".to_vec();
        data.extend_from_slice(b"AUDIO");
        let mut v1 = vec![0u8; 128];
        v1[..3].copy_from_slice(b"TAG");
        data.extend_from_slice(&v1);
        fs::write(&path, &data).unwrap();
        assert!(remove_id3v2(&path).unwrap());
        let after = fs::read(&path).unwrap();
        assert_eq!(&after[..5], b"AUDIO");
        assert!(remove_id3v1(&path).unwrap());
        assert_eq!(fs::read(&path).unwrap(), b"AUDIO");
        let _ = fs::remove_file(path);
    }
}
