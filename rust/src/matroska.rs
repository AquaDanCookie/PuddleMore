//! Small EBML/Matroska reader and writer used for MKV/MKA/WebM tags.
//!
//! Matroska stores metadata in two places that matter to a tag editor:
//! the Segment Info `Title` element and the global `Tags` element. Both are
//! exposed here, while track/chapter-targeted tags are preserved verbatim.

use anyhow::{anyhow, Result};
use memmap2::Mmap;
use std::fs;
use std::fs::OpenOptions;
use std::io::{Read, Write};
use std::io::{Seek, SeekFrom};
use std::path::{Path, PathBuf};

const SEGMENT_ID: u64 = 0x1853_8067;
const SEEKHEAD_ID: u64 = 0x114D_9B74;
const INFO_ID: u64 = 0x1549_A966;
const SEGMENT_TITLE_ID: u64 = 0x7BA9;
const MUXINGAPP_ID: u64 = 0x4D80;
const WRITINGAPP_ID: u64 = 0x5741;
const DATEUTC_ID: u64 = 0x4461;
const TRACKS_ID: u64 = 0x1654_AE6B;
const TRACKENTRY_ID: u64 = 0xAE;
const TRACKTYPE_ID: u64 = 0x83;
const TRACKNAME_ID: u64 = 0x536E;
const LANGUAGE_ID: u64 = 0x22B5_9C;
const CODECID_ID: u64 = 0x86;
const CODECNAME_ID: u64 = 0x2586_88;
const AUDIO_ID: u64 = 0xE1;
const AUDIOSAMPLINGFREQ_ID: u64 = 0xB5;
const AUDIOBITDEPTH_ID: u64 = 0x6264;
const AUDIOCHANNELS_ID: u64 = 0x9F;
const DURATION_ID: u64 = 0x4489;
const CLUSTER_ID: u64 = 0x1F43_B675;
const TAGS_ID: u64 = 0x1254_C367;
const TAG_ID: u64 = 0x7373;
const TARGETS_ID: u64 = 0x63C0;
const SIMPLETAG_ID: u64 = 0x67C8;
const TAGNAME_ID: u64 = 0x45A3;
const TAGLANGUAGE_ID: u64 = 0x447A;
const TAGDEFAULT_ID: u64 = 0x4484;
const TAGSTRING_ID: u64 = 0x4487;
const ATTACHMENTS_ID: u64 = 0x1941_A469;
const ATTACHEDFILE_ID: u64 = 0x61A7;
const FILEMIMETYPE_ID: u64 = 0x4660;
const FILEDATA_ID: u64 = 0x465C;
const VOID_ID: u64 = 0xEC;
const CRC32_ID: u64 = 0xBF;

const TARGETED_UID_IDS: [u64; 4] = [0x63C5, 0x63C4, 0x63C9, 0x63C6];

#[derive(Clone, Debug, Default)]
pub struct AudioInfo {
    pub codec: String,
    pub frequency: f64,
    pub channels: u32,
    pub bits_per_sample: u32,
    pub duration_ms: f64,
    pub name: String,
    pub language: String,
    pub codec_name: String,
}

#[derive(Clone, Debug)]
pub struct Image {
    pub mime: String,
    pub data: Vec<u8>,
}

#[derive(Debug)]
pub struct MatroskaFile {
    path: PathBuf,
    data: Mmap,
    seg_id_start: usize,
    seg_payload: usize,
    seg_end: usize,
    info_region: Option<(usize, usize, usize)>,
    tags_region: Option<(usize, usize)>,
    pub segment_title: String,
    pub muxing_app: String,
    pub writing_app: String,
    pub date_utc: String,
    pub audio: AudioInfo,
    pub images: Vec<Image>,
    /// Raw non-image AttachedFile elements, preserved across saves.
    attachment_blobs: Vec<Vec<u8>>,
    /// Global simple tags, preserving the Matroska tag name.
    pub global_tags: Vec<(String, Vec<String>)>,
    targeted_tags: Vec<Vec<u8>>,
}

#[derive(Clone, Copy)]
struct Elem {
    id: u64,
    start: usize,
    payload: usize,
    end: usize,
}

fn read_id(data: &[u8], pos: usize) -> Option<(u64, usize)> {
    let first = *data.get(pos)?;
    let mut len = 1u8;
    let mut mask = 0x80u8;
    while len < 8 && first & mask == 0 {
        mask >>= 1;
        len += 1;
    }
    if pos + len as usize > data.len() {
        return None;
    }
    let mut value = 0u64;
    for i in 0..len as usize {
        value = (value << 8) | data[pos + i] as u64;
    }
    Some((value, pos + len as usize))
}

fn read_size(data: &[u8], pos: usize) -> Option<(i64, usize)> {
    let first = *data.get(pos)?;
    let mut len = 1u8;
    let mut mask = 0x80u8;
    while len < 8 && first & mask == 0 {
        mask >>= 1;
        len += 1;
    }
    if pos + len as usize > data.len() {
        return None;
    }
    let mut value = (first & (mask - 1)) as u64;
    for i in 1..len as usize {
        value = (value << 8) | data[pos + i] as u64;
    }
    let unknown = (1u64 << (7 * len as u32)) - 1;
    if value == unknown {
        Some((-1, pos + len as usize))
    } else {
        Some((value as i64, pos + len as usize))
    }
}

fn flat(data: &[u8], start: usize, end: usize) -> Vec<Elem> {
    let mut out = Vec::new();
    let mut pos = start;
    while pos < end {
        let Some((id, after_id)) = read_id(data, pos) else {
            break;
        };
        let Some((size, payload)) = read_size(data, after_id) else {
            break;
        };
        if size < 0 {
            out.push(Elem {
                id,
                start: pos,
                payload,
                end,
            });
            break;
        }
        let elem_end = payload.saturating_add(size as usize);
        if elem_end > end {
            break;
        }
        out.push(Elem {
            id,
            start: pos,
            payload,
            end: elem_end,
        });
        pos = elem_end;
    }
    out
}

fn enc_size(size: usize) -> Vec<u8> {
    let mut width = 1usize;
    while size > (1usize << (7 * width)) - 2 && width < 8 {
        width += 1;
    }
    let mut buf = vec![0u8; width];
    let mut value = size;
    for i in (0..width).rev() {
        buf[i] = (value & 0xff) as u8;
        value >>= 8;
    }
    buf[0] |= 0x80 >> (width - 1);
    buf
}

fn encode_element(id: u64, data: &[u8]) -> Vec<u8> {
    let mut id_width = 1usize;
    while id >> (8 * id_width) != 0 {
        id_width += 1;
    }
    let mut out = Vec::with_capacity(id_width + 8 + data.len());
    out.extend_from_slice(&id.to_be_bytes()[8 - id_width..]);
    out.extend_from_slice(&enc_size(data.len()));
    out.extend_from_slice(data);
    out
}

fn encode_text(id: u64, text: &str) -> Vec<u8> {
    encode_element(id, text.as_bytes())
}

fn decode_text(data: &[u8]) -> String {
    String::from_utf8_lossy(data).into_owned()
}

fn to_float(raw: &[u8]) -> f64 {
    if let Ok(text) = std::str::from_utf8(raw) {
        if let Ok(value) = text.trim().parse::<f64>() {
            return value;
        }
    }
    if raw.len() == 4 {
        return f32::from_be_bytes([raw[0], raw[1], raw[2], raw[3]]) as f64;
    }
    if raw.len() == 8 {
        return f64::from_be_bytes([
            raw[0], raw[1], raw[2], raw[3], raw[4], raw[5], raw[6], raw[7],
        ]);
    }
    0.0
}

pub fn looks_like_matroska(path: &Path) -> bool {
    let Ok(mut file) = fs::File::open(path) else {
        return false;
    };
    let mut header = [0u8; 4];
    file.read_exact(&mut header).is_ok() && header == [0x1a, 0x45, 0xdf, 0xa3]
}

pub fn canonical_field(name: &str) -> String {
    match name.to_ascii_uppercase().as_str() {
        "TITLE" => "title",
        "ARTIST" => "artist",
        "ALBUM" => "album",
        "ALBUM_ARTIST" | "ALBUMARTIST" => "albumartist",
        "COMPOSER" => "composer",
        "COMMENT" => "comment",
        "DATE" | "YEAR" => "year",
        "GENRE" => "genre",
        "PART_NUMBER" | "TRACKNUMBER" => "track",
        "TOTAL_PARTS" => "totaltracks",
        "DISCNUMBER" | "DISC" => "disc",
        "DISCTOTAL" | "TOTAL_DISCS" => "totaldiscs",
        "DESCRIPTION" => "description",
        "LYRICS" => "lyrics",
        "COPYRIGHT" => "copyright",
        "ENCODED_BY" => "encodedby",
        "BPM" => "bpm",
        "ISRC" => "isrc",
        "MOOD" => "mood",
        "LABEL" | "PUBLISHER" => "organization",
        "CONDUCTOR" => "conductor",
        "LYRICIST" => "lyricist",
        "ARRANGER" => "arranger",
        "ORIGINAL_ALBUM" => "originalalbum",
        "ORIGINAL_ARTIST" => "originalartist",
        "ORIGINAL_RELEASE_DATE" => "originalyear",
        "SUBTITLE" => "subtitle",
        "GROUPING" => "grouping",
        "INITIAL_KEY" => "initialkey",
        "LANGUAGE" => "language",
        other => return other.to_ascii_lowercase(),
    }
    .to_string()
}

pub fn matroska_name(field: &str) -> String {
    match field.to_ascii_lowercase().as_str() {
        "title" => "TITLE",
        "artist" => "ARTIST",
        "album" => "ALBUM",
        "albumartist" => "ALBUM_ARTIST",
        "composer" => "COMPOSER",
        "comment" => "COMMENT",
        "year" => "DATE",
        "genre" => "GENRE",
        "track" => "PART_NUMBER",
        "totaltracks" => "TOTAL_PARTS",
        "disc" => "DISCNUMBER",
        "totaldiscs" => "DISCTOTAL",
        "description" => "DESCRIPTION",
        "lyrics" => "LYRICS",
        "copyright" => "COPYRIGHT",
        "encodedby" => "ENCODED_BY",
        "bpm" => "BPM",
        "isrc" => "ISRC",
        "mood" => "MOOD",
        "organization" => "LABEL",
        "conductor" => "CONDUCTOR",
        "lyricist" => "LYRICIST",
        "arranger" => "ARRANGER",
        "originalalbum" => "ORIGINAL_ALBUM",
        "originalartist" => "ORIGINAL_ARTIST",
        "originalyear" => "ORIGINAL_RELEASE_DATE",
        "subtitle" => "SUBTITLE",
        "grouping" => "GROUPING",
        "initialkey" => "INITIAL_KEY",
        "language" => "LANGUAGE",
        other => return other.to_ascii_uppercase(),
    }
    .to_string()
}

impl MatroskaFile {
    pub fn read(path: &Path) -> Result<Self> {
        let file = fs::File::open(path)?;
        let data = unsafe { Mmap::map(&file)? };
        Self::from_mmap(path.to_path_buf(), data)
    }

    pub fn from_mmap(path: PathBuf, data: Mmap) -> Result<Self> {
        let mut pos = 0usize;
        let mut segment = None;
        while pos < data.len() {
            let Some((id, after_id)) = read_id(&data, pos) else {
                break;
            };
            let Some((size, payload)) = read_size(&data, after_id) else {
                break;
            };
            if id == SEGMENT_ID {
                let end = if size < 0 {
                    data.len()
                } else {
                    payload.saturating_add(size as usize).min(data.len())
                };
                segment = Some((pos, payload, end));
                break;
            }
            if size < 0 {
                break;
            }
            pos = payload.saturating_add(size as usize);
        }
        let Some((seg_id_start, seg_payload, seg_end)) = segment else {
            return Err(anyhow!("No Matroska segment found"));
        };

        let mut out = MatroskaFile {
            path,
            data,
            seg_id_start,
            seg_payload,
            seg_end,
            info_region: None,
            tags_region: None,
            segment_title: String::new(),
            muxing_app: String::new(),
            writing_app: String::new(),
            date_utc: String::new(),
            audio: AudioInfo::default(),
            images: Vec::new(),
            attachment_blobs: Vec::new(),
            global_tags: Vec::new(),
            targeted_tags: Vec::new(),
        };

        let mut global_containers = Vec::new();
        for child in flat(&out.data, seg_payload, seg_end) {
            match child.id {
                TAGS_ID => {
                    let mut has_global = false;
                    for tag in flat(&out.data, child.payload, child.end) {
                        if tag.id != TAG_ID {
                            continue;
                        }
                        if tag_is_targeted(&out.data, tag.payload, tag.end) {
                            out.targeted_tags
                                .push(out.data[tag.start..tag.end].to_vec());
                        } else {
                            has_global = true;
                            out.read_simple_tags(tag.payload, tag.end);
                        }
                    }
                    if has_global {
                        global_containers.push((child.start, child.end));
                    }
                }
                INFO_ID => {
                    out.info_region = Some((child.start, child.payload, child.end));
                    out.read_info(child.payload, child.end);
                }
                TRACKS_ID => out.read_tracks(child.payload, child.end),
                ATTACHMENTS_ID => out.read_attachments(child.payload, child.end),
                _ => {}
            }
        }

        if global_containers.len() == 1 {
            let (start, mut end) = global_containers[0];
            for void in flat(&out.data, end, seg_end) {
                if void.id == VOID_ID {
                    end = void.end;
                } else {
                    break;
                }
            }
            out.tags_region = Some((start, end));
        }

        Ok(out)
    }

    fn read_simple_tags(&mut self, payload: usize, end: usize) {
        for simple in flat(&self.data, payload, end) {
            if simple.id != SIMPLETAG_ID {
                continue;
            }
            let mut name = None;
            let mut value = String::new();
            for item in flat(&self.data, simple.payload, simple.end) {
                match item.id {
                    TAGNAME_ID => name = Some(decode_text(&self.data[item.payload..item.end])),
                    TAGSTRING_ID => {
                        value = decode_text(&self.data[item.payload..item.end]);
                    }
                    _ => {}
                }
            }
            if let Some(name) = name {
                if let Some((_, values)) = self.global_tags.iter_mut().find(|(n, _)| n == &name) {
                    values.push(value);
                } else {
                    self.global_tags.push((name, vec![value]));
                }
            }
        }
    }

    fn read_info(&mut self, payload: usize, end: usize) {
        for elem in flat(&self.data, payload, end) {
            match elem.id {
                DURATION_ID => {
                    let value = to_float(&self.data[elem.payload..elem.end]);
                    if value > 0.0 {
                        self.audio.duration_ms = value;
                    }
                }
                SEGMENT_TITLE_ID => {
                    self.segment_title = decode_text(&self.data[elem.payload..elem.end]);
                }
                MUXINGAPP_ID => {
                    self.muxing_app = decode_text(&self.data[elem.payload..elem.end]);
                }
                WRITINGAPP_ID => {
                    self.writing_app = decode_text(&self.data[elem.payload..elem.end]);
                }
                DATEUTC_ID => {
                    if elem.end - elem.payload == 8 {
                        let ns = i64::from_be_bytes(
                            self.data[elem.payload..elem.end]
                                .try_into()
                                .unwrap_or([0; 8]),
                        );
                        if ns != 0 {
                            self.date_utc = format_ns_since_2001(ns);
                        }
                    }
                }
                _ => {}
            }
        }
    }

    fn read_tracks(&mut self, payload: usize, end: usize) {
        for track in flat(&self.data, payload, end) {
            if track.id != TRACKENTRY_ID {
                continue;
            }
            let mut info = AudioInfo::default();
            let mut track_type = None;
            for item in flat(&self.data, track.payload, track.end) {
                match item.id {
                    TRACKTYPE_ID => {
                        track_type = Some(u32::from_be_bytes(
                            int_bytes(&self.data[item.payload..item.end], 4)
                                .try_into()
                                .unwrap_or([0; 4]),
                        ));
                    }
                    CODECID_ID => {
                        info.codec = decode_text(&self.data[item.payload..item.end]);
                    }
                    TRACKNAME_ID => {
                        info.name = decode_text(&self.data[item.payload..item.end]);
                    }
                    LANGUAGE_ID => {
                        info.language = decode_text(&self.data[item.payload..item.end]);
                    }
                    CODECNAME_ID => {
                        info.codec_name = decode_text(&self.data[item.payload..item.end]);
                    }
                    AUDIO_ID => {
                        for audio in flat(&self.data, item.payload, item.end) {
                            match audio.id {
                                AUDIOSAMPLINGFREQ_ID => {
                                    info.frequency = to_float(&self.data[audio.payload..audio.end]);
                                }
                                AUDIOCHANNELS_ID => {
                                    info.channels = u32::from_be_bytes(
                                        int_bytes(&self.data[audio.payload..audio.end], 4)
                                            .try_into()
                                            .unwrap_or([0; 4]),
                                    );
                                }
                                AUDIOBITDEPTH_ID => {
                                    info.bits_per_sample = u32::from_be_bytes(
                                        int_bytes(&self.data[audio.payload..audio.end], 4)
                                            .try_into()
                                            .unwrap_or([0; 4]),
                                    );
                                }
                                _ => {}
                            }
                        }
                    }
                    _ => {}
                }
            }
            if track_type == Some(2) {
                let duration = self.audio.duration_ms;
                info.duration_ms = duration;
                self.audio = info;
                return;
            }
        }
    }

    fn read_attachments(&mut self, payload: usize, end: usize) {
        for file in flat(&self.data, payload, end) {
            if file.id != ATTACHEDFILE_ID {
                continue;
            }
            let mut mime = String::new();
            let mut bytes = Vec::new();
            for item in flat(&self.data, file.payload, file.end) {
                match item.id {
                    FILEMIMETYPE_ID => {
                        mime = decode_text(&self.data[item.payload..item.end]);
                    }
                    FILEDATA_ID => {
                        bytes = self.data[item.payload..item.end].to_vec();
                    }
                    _ => {}
                }
            }
            if mime.starts_with("image/") && !bytes.is_empty() {
                self.images.push(Image { mime, data: bytes });
            } else {
                self.attachment_blobs
                    .push(self.data[file.start..file.end].to_vec());
            }
        }
    }

    pub fn save(
        &mut self,
        tags: &[(String, Vec<String>)],
        title: Option<&str>,
        images: Option<Vec<Image>>,
    ) -> Result<()> {
        let new_info = match title {
            Some(title) if title != self.segment_title => self.build_info(title),
            _ => None,
        };
        let new_tags = self.build_tags(tags);
        let attachments = images.as_ref().map(|imgs| self.build_attachments(imgs));

        if new_info.is_none() && images.is_none() {
            if let Some((start, end)) = self.tags_region {
                let remaining = end.saturating_sub(start + new_tags.len());
                if end - start >= new_tags.len() && remaining != 1 {
                    let tail = if remaining == 0 {
                        Vec::new()
                    } else {
                        pad_total(remaining)
                    };
                    let mut blob = new_tags;
                    blob.extend_from_slice(&tail);
                    let mut file = OpenOptions::new().write(true).open(&self.path)?;
                    file.seek(SeekFrom::Start(start as u64))?;
                    file.write_all(&blob)?;
                    return Ok(());
                }
            }
        }

        let mut children = Vec::new();
        let mut tags_written = false;
        let mut attachments_written = false;
        for child in flat(&self.data, self.seg_payload, self.seg_end) {
            if matches!(child.id, SEEKHEAD_ID | CRC32_ID | VOID_ID) {
                continue;
            }
            if child.id == TAGS_ID {
                if !tags_written && !new_tags.is_empty() {
                    children.extend_from_slice(&new_tags);
                    tags_written = true;
                }
                continue;
            }
            if child.id == ATTACHMENTS_ID && attachments.is_some() {
                if !attachments_written {
                    if let Some(blob) = attachments.as_ref() {
                        if !blob.is_empty() {
                            children.extend_from_slice(blob);
                        }
                    }
                    attachments_written = true;
                }
                continue;
            }
            if child.id == CLUSTER_ID {
                if !tags_written && !new_tags.is_empty() {
                    children.extend_from_slice(&new_tags);
                    tags_written = true;
                }
                if !attachments_written {
                    if let Some(blob) = attachments.as_ref() {
                        if !blob.is_empty() {
                            children.extend_from_slice(blob);
                        }
                    }
                    attachments_written = true;
                }
            }
            if child.id == INFO_ID {
                if let Some(info) = new_info.as_ref() {
                    children.extend_from_slice(info);
                } else {
                    children.extend_from_slice(&self.data[child.start..child.end]);
                }
                continue;
            }
            children.extend_from_slice(&self.data[child.start..child.end]);
        }
        if !tags_written && !new_tags.is_empty() {
            children.extend_from_slice(&new_tags);
        }
        if !attachments_written {
            if let Some(blob) = attachments.as_ref() {
                if !blob.is_empty() {
                    children.extend_from_slice(blob);
                }
            }
        }

        let mut rebuilt = Vec::new();
        rebuilt.extend_from_slice(&self.data[..self.seg_id_start]);
        rebuilt.extend_from_slice(&encode_element(SEGMENT_ID, &children));
        self.write_data(&rebuilt)?;
        if let Some(title) = title {
            self.segment_title = title.to_string();
        }
        Ok(())
    }

    fn build_tags(&self, tags: &[(String, Vec<String>)]) -> Vec<u8> {
        let mut inner = Vec::new();
        for (name, values) in tags {
            for value in values {
                let mut simple = Vec::new();
                simple.extend_from_slice(&encode_text(TAGNAME_ID, name));
                simple.extend_from_slice(&encode_text(TAGLANGUAGE_ID, "und"));
                simple.extend_from_slice(&encode_element(
                    TAGDEFAULT_ID,
                    &[if value.is_empty() { 0 } else { 1 }],
                ));
                simple.extend_from_slice(&encode_text(TAGSTRING_ID, value));
                inner.extend_from_slice(&encode_element(SIMPLETAG_ID, &simple));
            }
        }
        let mut parts = Vec::new();
        if !inner.is_empty() {
            parts.extend_from_slice(&encode_element(TAG_ID, &inner));
        }
        for raw in &self.targeted_tags {
            parts.extend_from_slice(raw);
        }
        if parts.is_empty() {
            Vec::new()
        } else {
            encode_element(TAGS_ID, &parts)
        }
    }

    fn build_info(&self, title: &str) -> Option<Vec<u8>> {
        let (_, payload, end) = self.info_region?;
        let mut inner = Vec::new();
        for elem in flat(&self.data, payload, end) {
            if elem.id == SEGMENT_TITLE_ID {
                continue;
            }
            inner.extend_from_slice(&self.data[elem.start..elem.end]);
        }
        if !title.is_empty() {
            inner.extend_from_slice(&encode_text(SEGMENT_TITLE_ID, title));
        }
        Some(encode_element(INFO_ID, &inner))
    }

    fn build_attachments(&self, images: &[Image]) -> Vec<u8> {
        if images.is_empty() && self.attachment_blobs.is_empty() {
            return Vec::new();
        }
        let mut inner = Vec::new();
        for blob in &self.attachment_blobs {
            inner.extend_from_slice(blob);
        }
        for (index, image) in images.iter().enumerate() {
            let extension = match image.mime.as_str() {
                "image/jpeg" => "jpg",
                "image/tiff" => "tif",
                other => other
                    .split('/')
                    .next_back()
                    .filter(|e| !e.is_empty())
                    .unwrap_or("bin"),
            };
            let name = format!("cover_{}.{}", index + 1, extension);
            let uid = 0x6edc_0001u32.wrapping_add(simple_hash(&image.data) & 0xfffff);
            let mut file = Vec::new();
            file.extend_from_slice(&encode_text(0x466E, &name));
            file.extend_from_slice(&encode_element(0x46AE, &uid.to_be_bytes()));
            file.extend_from_slice(&encode_text(FILEMIMETYPE_ID, &image.mime));
            file.extend_from_slice(&encode_element(FILEDATA_ID, &image.data));
            inner.extend_from_slice(&encode_element(ATTACHEDFILE_ID, &file));
        }
        encode_element(ATTACHMENTS_ID, &inner)
    }

    fn write_data(&self, data: &[u8]) -> Result<()> {
        let parent = self.path.parent().unwrap_or_else(|| Path::new("."));
        let tmp = parent.join(format!(
            ".puddlemore_tmp_{}_{}",
            std::process::id(),
            data.len()
        ));
        {
            let mut file = fs::File::create(&tmp)?;
            file.write_all(data)?;
        }
        if let Ok(metadata) = fs::metadata(&self.path) {
            let _ = fs::set_permissions(&tmp, metadata.permissions());
        }
        fs::rename(&tmp, &self.path)?;
        Ok(())
    }
}

fn int_bytes(raw: &[u8], min_width: usize) -> Vec<u8> {
    let mut out = vec![0u8; min_width.saturating_sub(raw.len())];
    out.extend_from_slice(raw);
    out
}

fn simple_hash(data: &[u8]) -> u32 {
    let mut hash = 2166136261u32;
    for byte in data {
        hash ^= *byte as u32;
        hash = hash.wrapping_mul(16777619);
    }
    hash
}

fn tag_is_targeted(data: &[u8], payload: usize, end: usize) -> bool {
    for elem in flat(data, payload, end) {
        if elem.id != TARGETS_ID {
            continue;
        }
        for target in flat(data, elem.payload, elem.end) {
            if TARGETED_UID_IDS.contains(&target.id) {
                return true;
            }
        }
    }
    false
}

fn pad_total(total: usize) -> Vec<u8> {
    let mut payload = 0usize;
    loop {
        let element = encode_element(VOID_ID, &vec![0u8; payload]);
        if element.len() >= total {
            return element;
        }
        payload += 1;
    }
}

fn format_ns_since_2001(ns: i64) -> String {
    // 2001-01-01T00:00:00Z in Unix seconds.
    const EPOCH: i64 = 978_307_200;
    let secs = EPOCH + ns / 1_000_000_000;
    let days = secs.div_euclid(86_400);
    let rem = secs.rem_euclid(86_400);
    let (year, month, day) = civil_from_days(days);
    format!(
        "{:04}-{:02}-{:02} {:02}:{:02}:{:02} UTC",
        year,
        month,
        day,
        rem / 3600,
        (rem % 3600) / 60,
        rem % 60
    )
}

// Howard Hinnant's civil_from_days.
fn civil_from_days(days: i64) -> (i64, u32, u32) {
    let z = days + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    (if m <= 2 { y + 1 } else { y }, m, d)
}
