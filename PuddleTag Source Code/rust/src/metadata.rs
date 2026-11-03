//! Format-neutral tag model used by the Rust UI and CLI.
//!
//! Most formats go through TagLib because its property map exposes arbitrary
//! fields. Matroska has its own reader/writer for Segment Info titles and
//! targeted tags. TrueAudio files with APEv2 tags get a native fallback.

use crate::aiff;
use crate::apev2;
use crate::matroska::{canonical_field, looks_like_matroska, matroska_name, MatroskaFile};
use crate::taglib;
use anyhow::{Context, Result};
use lofty::config::WriteOptions;
use lofty::file::{AudioFile, FileType, TaggedFileExt};
use lofty::probe::Probe;
use lofty::tag::{ItemKey, ItemValue, Tag, TagExt};
use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};
use walkdir::WalkDir;

const AUDIO_EXTENSIONS: &[&str] = &[
    "mp3", "mp2", "mp1", "flac", "ogg", "oga", "opus", "spx", "m4a", "mp4", "m4v", "m4b", "aac",
    "wav", "wave", "aif", "aiff", "aifc", "wv", "ape", "mpc", "mpp", "tta", "tak", "dsf", "dff",
    "ofr", "ofs", "mka", "mkv", "webm", "wma", "wmv", "asf",
];

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Backend {
    TagLib,
    Lofty,
    Matroska,
    Aiff,
    Ape,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ImageData {
    pub data: Vec<u8>,
    pub mime: String,
    pub description: String,
    pub image_type: u8,
}

#[derive(Clone, Debug)]
pub struct Track {
    pub path: PathBuf,
    pub fields: BTreeMap<String, Vec<String>>,
    pub original_fields: BTreeMap<String, Vec<String>>,
    pub dirty: bool,
    pub images: Vec<ImageData>,
    pub original_images: Vec<ImageData>,
    pub error: Option<String>,
    pub format: String,
    pub codec: String,
    pub length: String,
    pub bitrate: String,
    pub matroska_names: HashMap<String, String>,
    pub(crate) taglib_names: HashMap<String, String>,
    pub(crate) ape_names: HashMap<String, String>,
    pub(crate) segment_title_present: bool,
    pub(crate) segment_title_only: bool,
    pub(crate) backend: Backend,
}

impl Track {
    pub fn display(&self, field: &str) -> String {
        self.fields
            .get(field)
            .map(|values| values.join("\\\\"))
            .unwrap_or_default()
    }

    pub fn set_display(&mut self, field: &str, value: String) {
        if value.is_empty() {
            self.fields.remove(field);
        } else {
            let values = value
                .split("\\\\")
                .map(|part| part.to_string())
                .collect::<Vec<_>>();
            self.fields.insert(field.to_string(), values);
        }
        self.dirty = true;
    }

    pub fn is_read_only(field: &str) -> bool {
        field.starts_with("__") || read_only_fields().contains(&field)
    }
}

fn read_only_fields() -> &'static [&'static str] {
    &[
        "length",
        "bitrate",
        "codec",
        "filetype",
        "filename",
        "directory",
        "size",
        "modified",
        "frequency",
        "channels",
        "bitspersample",
    ]
}

pub fn is_audio_file(path: &Path) -> bool {
    path.extension()
        .and_then(|ext| ext.to_str())
        .map(|ext| AUDIO_EXTENSIONS.contains(&ext.to_ascii_lowercase().as_str()))
        .unwrap_or(false)
}

pub fn scan_paths(paths: &[PathBuf]) -> Vec<PathBuf> {
    scan_paths_in(paths, true)
}

/// Scan paths, optionally restricting folders to their immediate files.
pub fn scan_paths_in(paths: &[PathBuf], recursive: bool) -> Vec<PathBuf> {
    let mut files = Vec::new();
    for path in paths {
        if path.is_dir() {
            let walker = WalkDir::new(path).follow_links(false).sort_by_file_name();
            let walker = if recursive {
                walker
            } else {
                walker.max_depth(1)
            };
            for entry in walker.into_iter().filter_map(Result::ok) {
                if entry.file_type().is_file() && is_audio_file(entry.path()) {
                    files.push(entry.into_path());
                }
            }
        } else if path.is_file() && is_audio_file(path) {
            files.push(path.clone());
        }
    }
    files.sort();
    files.dedup();
    files
}

pub fn load_track(path: &Path) -> Track {
    let mut track = Track {
        path: path.to_path_buf(),
        fields: BTreeMap::new(),
        original_fields: BTreeMap::new(),
        dirty: false,
        images: Vec::new(),
        original_images: Vec::new(),
        error: None,
        format: String::new(),
        codec: String::new(),
        length: String::new(),
        bitrate: String::new(),
        matroska_names: HashMap::new(),
        taglib_names: HashMap::new(),
        ape_names: HashMap::new(),
        segment_title_present: false,
        segment_title_only: false,
        backend: Backend::Lofty,
    };

    if looks_like_matroska(path) {
        track.backend = Backend::Matroska;
        match load_matroska(&mut track) {
            Ok(()) => {}
            Err(err) => track.error = Some(format!("{err:#}")),
        }
    } else {
        let ext = path
            .extension()
            .and_then(|ext| ext.to_str())
            .unwrap_or_default()
            .to_ascii_lowercase();
        let mut loaded = false;
        if matches!(ext.as_str(), "aif" | "aiff" | "aifc") {
            if let Ok(native) = aiff::read_native(path) {
                if !native.is_empty() {
                    // TagLib only sees ID3 in AIFF. Keep ID3 values when
                    // present and fill missing fields from the format's native
                    // text chunks; saves update both stores.
                    match taglib::read(path) {
                        Ok(result) => apply_taglib(&mut track, result),
                        Err(_) => {
                            track.format = filetype_label(&ext);
                            track.codec = codec_for_extension(&ext);
                        }
                    }
                    for (field, values) in native {
                        if field == "comment" {
                            // AIFF allows more than one ANNO chunk. Keep those
                            // annotations even when ID3 also has a comment.
                            let comments = track.fields.entry(field).or_default();
                            for value in values {
                                if !comments.contains(&value) {
                                    comments.push(value);
                                }
                            }
                        } else {
                            track.fields.entry(field).or_insert(values);
                        }
                    }
                    track.backend = Backend::Aiff;
                    loaded = true;
                }
            }
        }
        if ext == "tta" && apev2::has_tag(path) {
            // TagLib's TrueAudio reader only understands ID3. When an APEv2
            // tag is present instead, read that natively; otherwise a save
            // would add a second, conflicting ID3 tag.
            let probe = taglib::read(path);
            let taglib_is_empty = probe
                .as_ref()
                .map(|result| result.fields.is_empty())
                .unwrap_or(true);
            if taglib_is_empty {
                if let Ok(ape) = apev2::read(path) {
                    match probe {
                        Ok(result) => apply_taglib(&mut track, result),
                        Err(_) => {
                            track.format = filetype_label(&ext);
                            track.codec = codec_for_extension(&ext);
                        }
                    }
                    // Keep the audio/file properties, replace tag fields.
                    track.fields.retain(|field, _| field.starts_with("__"));
                    track.fields.extend(ape.fields);
                    track.ape_names = ape.names;
                    track.backend = Backend::Ape;
                    loaded = true;
                }
            }
        }
        if !loaded {
            match taglib::read(path) {
                Ok(result) => {
                    track.backend = Backend::TagLib;
                    apply_taglib(&mut track, result);
                }
                Err(taglib_error) => match load_lofty(&mut track) {
                    Ok(()) => {
                        track.backend = Backend::Lofty;
                    }
                    Err(lofty_error) => {
                        track.error =
                            Some(format!("TagLib: {taglib_error:#}; Lofty: {lofty_error:#}"));
                    }
                },
            }
        }
    }

    add_file_fields(&mut track);
    track.original_fields = track.fields.clone();
    track.original_images = track.images.clone();
    track
}

fn load_matroska(track: &mut Track) -> Result<()> {
    let file = MatroskaFile::read(&track.path)?;
    track.format = filetype_label(
        track
            .path
            .extension()
            .and_then(|e| e.to_str())
            .unwrap_or(""),
    );
    track.codec = codec_from_matroska(&file.audio.codec);
    track.length = format_seconds(file.audio.duration_ms / 1000.0);
    track.bitrate = if file.audio.duration_ms > 0.0 {
        let size = std::fs::metadata(&track.path).map(|m| m.len()).unwrap_or(0);
        format!(
            "{} kb/s",
            (size as f64 * 8.0 / file.audio.duration_ms) as u64
        )
    } else {
        String::new()
    };
    track.segment_title_present = !file.segment_title.is_empty();
    track.segment_title_only = false;
    track.images = file
        .images
        .iter()
        .map(|image| ImageData {
            data: image.data.clone(),
            mime: image.mime.clone(),
            description: String::new(),
            image_type: 3,
        })
        .collect();

    if !file.segment_title.is_empty() {
        track
            .fields
            .insert("title".into(), vec![file.segment_title.clone()]);
    }

    let mut has_title_tag = false;
    for (name, values) in &file.global_tags {
        let field = canonical_field(name);
        if field == "title" {
            has_title_tag = true;
        }
        track
            .matroska_names
            .entry(field.clone())
            .or_insert_with(|| name.clone());
        if field == "track" && values.len() == 1 && values[0].contains('/') {
            let (number, total) = values[0].split_once('/').unwrap();
            track
                .fields
                .insert("track".into(), vec![number.trim().to_string()]);
            track
                .fields
                .insert("totaltracks".into(), vec![total.trim().to_string()]);
            track
                .matroska_names
                .entry("totaltracks".into())
                .or_insert_with(|| "TOTAL_PARTS".into());
        } else {
            track.fields.insert(field, values.clone());
        }
    }
    track.segment_title_only = !file.segment_title.is_empty() && !has_title_tag;

    track
        .fields
        .insert("__muxingapp".into(), vec![file.muxing_app.clone()]);
    track
        .fields
        .insert("__writingapp".into(), vec![file.writing_app.clone()]);
    track
        .fields
        .insert("__dateutc".into(), vec![file.date_utc.clone()]);
    track.fields.insert(
        "__frequency".into(),
        vec![format!("{:.0}", file.audio.frequency)],
    );
    track
        .fields
        .insert("__channels".into(), vec![file.audio.channels.to_string()]);
    track.fields.insert(
        "__bitspersample".into(),
        vec![file.audio.bits_per_sample.to_string()],
    );
    if !file.audio.name.is_empty() {
        track
            .fields
            .insert("__trackname".into(), vec![file.audio.name.clone()]);
    }
    Ok(())
}

fn load_lofty(track: &mut Track) -> Result<()> {
    let tagged = Probe::open(&track.path)
        .with_context(|| format!("opening {}", track.path.display()))?
        .read()
        .with_context(|| format!("reading {}", track.path.display()))?;
    let file_type = tagged.file_type();
    track.format = format_label(file_type, &track.path);
    track.codec = codec_label(file_type);
    let props = tagged.properties();
    track.length = format_seconds(props.duration().as_secs_f64());
    if let Some(bitrate) = props.audio_bitrate() {
        track.bitrate = format!("{} kb/s", bitrate / 1000);
    }

    let mut tag_fields: BTreeMap<String, Vec<String>> = BTreeMap::new();
    if let Some(tag) = tagged.primary_tag().or_else(|| tagged.first_tag()) {
        for item in tag.items() {
            if let ItemValue::Text(value) | ItemValue::Locator(value) = item.value() {
                tag_fields
                    .entry(item_field_name(item.key()))
                    .or_default()
                    .push(value.clone());
            }
        }
    }
    track.fields.extend(tag_fields);
    Ok(())
}

fn apply_taglib(track: &mut Track, result: taglib::ReadResult) {
    let ext = track
        .path
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or_default();
    track.format = filetype_label(ext);
    track.codec = codec_for_extension(ext);
    track.length = format_seconds(result.length_seconds);
    track.bitrate = if result.bitrate > 0 {
        format!("{} kb/s", result.bitrate)
    } else {
        String::new()
    };
    track.fields = result.fields;
    track.taglib_names = result.property_names;
    track.images = result.pictures;
    if result.samplerate > 0 {
        track
            .fields
            .insert("__frequency".into(), vec![result.samplerate.to_string()]);
    }
    if result.channels > 0 {
        track
            .fields
            .insert("__channels".into(), vec![result.channels.to_string()]);
    }
}

pub fn save_track(track: &mut Track) -> Result<()> {
    match track.backend {
        Backend::TagLib => save_taglib(track),
        Backend::Matroska => save_matroska(track),
        Backend::Aiff => save_aiff(track),
        Backend::Ape => save_ape(track),
        Backend::Lofty => save_lofty(track),
    }
}

fn save_taglib(track: &mut Track) -> Result<()> {
    taglib::write(
        &track.path,
        &track.fields,
        &track.taglib_names,
        Some(&track.images),
    )?;
    let refreshed = load_track(&track.path);
    *track = refreshed;
    Ok(())
}

fn save_ape(track: &mut Track) -> Result<()> {
    apev2::write(&track.path, &track.fields, &track.ape_names)?;
    let refreshed = load_track(&track.path);
    *track = refreshed;
    Ok(())
}

fn save_aiff(track: &mut Track) -> Result<()> {
    aiff::write_native(&track.path, &track.fields)?;
    taglib::write(
        &track.path,
        &track.fields,
        &track.taglib_names,
        Some(&track.images),
    )?;
    let refreshed = load_track(&track.path);
    *track = refreshed;
    Ok(())
}

fn save_matroska(track: &mut Track) -> Result<()> {
    let mut file = MatroskaFile::read(&track.path)?;
    let mut tags = Vec::new();
    for (field, values) in &track.fields {
        if field.starts_with("__") {
            continue;
        }
        if field == "title" && track.segment_title_only {
            continue;
        }
        let name = track
            .matroska_names
            .get(field)
            .cloned()
            .unwrap_or_else(|| matroska_name(field));
        tags.push((name, values.clone()));
    }

    let title_value = track
        .fields
        .get("title")
        .and_then(|values| values.first())
        .cloned()
        .unwrap_or_default();
    let new_title = if track.segment_title_present || track.segment_title_only {
        Some(title_value.as_str())
    } else {
        None
    };
    let images = (track.images != track.original_images).then(|| {
        track
            .images
            .iter()
            .map(|image| crate::matroska::Image {
                mime: image.mime.clone(),
                data: image.data.clone(),
            })
            .collect()
    });
    file.save(&tags, new_title, images)?;

    let refreshed = load_track(&track.path);
    track.fields = refreshed.fields;
    track.original_fields = refreshed.original_fields;
    track.matroska_names = refreshed.matroska_names;
    track.segment_title_present = refreshed.segment_title_present;
    track.segment_title_only = refreshed.segment_title_only;
    track.error = refreshed.error;
    track.dirty = false;
    Ok(())
}

fn save_lofty(track: &mut Track) -> Result<()> {
    let tagged = Probe::open(&track.path)?.read()?;
    let tag_type = tagged.primary_tag_type();
    let mut tag = tagged
        .primary_tag()
        .or_else(|| tagged.first_tag())
        .cloned()
        .unwrap_or_else(|| Tag::new(tag_type));

    let original_fields: Vec<String> = track
        .original_fields
        .keys()
        .filter(|field| !field.starts_with("__"))
        .cloned()
        .collect();
    for field in original_fields {
        if let Some(key) = item_key_for_field(&field) {
            tag.remove_key(key);
        }
    }

    for (field, values) in &track.fields {
        if field.starts_with("__") {
            continue;
        }
        let Some(key) = item_key_for_field(field) else {
            continue;
        };
        for value in values {
            if !value.is_empty() {
                tag.push(lofty::tag::TagItem::new(
                    key,
                    ItemValue::Text(value.clone()),
                ));
            }
        }
    }

    tag.save_to_path(&track.path, WriteOptions::default())?;
    track.original_fields = track.fields.clone();
    track.dirty = false;
    Ok(())
}

fn add_file_fields(track: &mut Track) {
    let path = &track.path;
    let filename = path
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or_default()
        .to_string();
    let ext = path
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or_default()
        .to_string();
    let dir = path
        .parent()
        .and_then(|p| p.to_str())
        .unwrap_or_default()
        .to_string();
    let size = std::fs::metadata(path).map(|m| m.len()).unwrap_or(0);
    let filetype = if track.format.is_empty() {
        filetype_label(&ext)
    } else {
        track.format.clone()
    };
    track
        .fields
        .insert("__filename".into(), vec![filename.clone()]);
    track.fields.insert("__ext".into(), vec![ext]);
    track
        .fields
        .insert("__filetype".into(), vec![filetype.clone()]);
    if track.format.is_empty() {
        track.format = filetype;
    }
    track.fields.insert("__dirpath".into(), vec![dir]);
    track.fields.insert("__size".into(), vec![size.to_string()]);
    track
        .fields
        .insert("__length".into(), vec![track.length.clone()]);
    track
        .fields
        .insert("__bitrate".into(), vec![track.bitrate.clone()]);
    track
        .fields
        .insert("__codec".into(), vec![track.codec.clone()]);
    track
        .fields
        .insert("__num_images".into(), vec![track.images.len().to_string()]);
    if let Some(image) = track.images.first() {
        track
            .fields
            .insert("__image_mimetype".into(), vec![image.mime.clone()]);
        track
            .fields
            .insert("__image_size".into(), vec![image.data.len().to_string()]);
    }
}

fn item_field_name(key: ItemKey) -> String {
    match key {
        ItemKey::TrackTitle => "title".into(),
        ItemKey::TrackArtist | ItemKey::TrackArtists => "artist".into(),
        ItemKey::AlbumTitle => "album".into(),
        ItemKey::AlbumArtist | ItemKey::AlbumArtists => "albumartist".into(),
        ItemKey::TrackSubtitle | ItemKey::SetSubtitle => "subtitle".into(),
        ItemKey::Comment => "comment".into(),
        ItemKey::Description => "description".into(),
        ItemKey::Genre => "genre".into(),
        ItemKey::TrackNumber => "track".into(),
        ItemKey::TrackTotal => "totaltracks".into(),
        ItemKey::DiscNumber => "disc".into(),
        ItemKey::DiscTotal => "totaldiscs".into(),
        ItemKey::Year | ItemKey::RecordingDate => "year".into(),
        ItemKey::OriginalReleaseDate => "originalyear".into(),
        ItemKey::ReleaseDate => "releasedate".into(),
        ItemKey::Composer => "composer".into(),
        ItemKey::Conductor => "conductor".into(),
        ItemKey::Lyricist | ItemKey::Writer => "lyricist".into(),
        ItemKey::Arranger => "arranger".into(),
        ItemKey::Publisher | ItemKey::Label => "organization".into(),
        ItemKey::CatalogNumber => "catalognumber".into(),
        ItemKey::Isrc => "isrc".into(),
        ItemKey::Bpm | ItemKey::IntegerBpm => "bpm".into(),
        ItemKey::Mood => "mood".into(),
        ItemKey::InitialKey => "initialkey".into(),
        ItemKey::Language => "language".into(),
        ItemKey::Lyrics | ItemKey::UnsyncLyrics => "lyrics".into(),
        ItemKey::CopyrightMessage => "copyright".into(),
        ItemKey::EncodedBy | ItemKey::EncoderSoftware => "encodedby".into(),
        ItemKey::EncoderSettings => "encodersettings".into(),
        ItemKey::ContentGroup | ItemKey::AppleId3v2ContentGroup => "grouping".into(),
        ItemKey::Work => "work".into(),
        ItemKey::Movement => "movementname".into(),
        ItemKey::MovementNumber => "movementnumber".into(),
        ItemKey::MovementTotal => "movementtotal".into(),
        ItemKey::PodcastDescription => "description".into(),
        ItemKey::PodcastUrl => "podcasturl".into(),
        ItemKey::PodcastSeriesCategory => "podcastcategory".into(),
        ItemKey::PodcastGlobalUniqueId => "podcastepisodeguid".into(),
        ItemKey::PodcastKeywords => "podcastkeywords".into(),
        ItemKey::FileOwner => "fileowner".into(),
        ItemKey::OriginalFileName => "filename".into(),
        ItemKey::OriginalMediaType => "mediatype".into(),
        ItemKey::ReplayGainAlbumGain => "albumgain".into(),
        ItemKey::ReplayGainAlbumPeak => "albumpeak".into(),
        ItemKey::ReplayGainTrackGain => "titlegain".into(),
        ItemKey::ReplayGainTrackPeak => "titlepeak".into(),
        other => camel_to_snake(&format!("{other:?}")),
    }
}

fn item_key_for_field(field: &str) -> Option<ItemKey> {
    for key in ItemKey::VARIANTS {
        if item_field_name(*key) == field {
            return Some(*key);
        }
    }
    None
}

fn camel_to_snake(value: &str) -> String {
    let mut out = String::new();
    for (index, ch) in value.chars().enumerate() {
        if ch.is_uppercase() && index > 0 {
            out.push('_');
        }
        out.push(ch.to_ascii_lowercase());
    }
    out
}

fn format_label(file_type: FileType, path: &Path) -> String {
    let ext = path
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or_default()
        .to_ascii_lowercase();
    let label = filetype_label(&ext);
    if label == ext.to_ascii_uppercase() {
        format!("{file_type:?}")
    } else {
        label
    }
}

pub fn filetype_label(ext: &str) -> String {
    match ext.to_ascii_lowercase().as_str() {
        "mp3" => "MP3",
        "mp2" | "mp1" => "MPEG Audio",
        "flac" => "FLAC",
        "ogg" => "Ogg Vorbis",
        "oga" => "Ogg Audio",
        "opus" => "Ogg Opus",
        "spx" => "Ogg Speex",
        "m4a" | "m4b" => "MP4 (AAC)",
        "mp4" | "m4v" => "MP4",
        "aac" => "AAC",
        "wav" | "wave" => "WAV",
        "aif" | "aiff" | "aifc" => "AIFF",
        "wv" => "WavPack",
        "ape" => "Monkey's Audio",
        "mpc" | "mpp" => "Musepack",
        "tta" => "TrueAudio",
        "tak" => "TAK",
        "dsf" => "DSD (DSF)",
        "dff" => "DSDIFF",
        "ofr" | "ofs" => "OptimFROG",
        "mka" => "Matroska Audio",
        "mkv" => "Matroska Video",
        "webm" => "WebM",
        "wma" | "wmv" => "WMA",
        "asf" => "ASF",
        "ac3" => "AC3",
        "dts" => "DTS",
        other => return other.to_ascii_uppercase(),
    }
    .to_string()
}

fn codec_label(file_type: FileType) -> String {
    match file_type {
        FileType::Flac => "FLAC",
        FileType::Mpeg => "MPEG",
        FileType::Mp4 => "AAC / ALAC",
        FileType::Vorbis => "Vorbis",
        FileType::Opus => "Opus",
        FileType::Speex => "Speex",
        FileType::Wav => "PCM",
        FileType::Ape => "Monkey's Audio",
        FileType::WavPack => "WavPack",
        FileType::Aiff => "AIFF",
        FileType::Aac => "AAC",
        FileType::Mpc => "Musepack",
        other => return format!("{other:?}"),
    }
    .to_string()
}

fn codec_for_extension(ext: &str) -> String {
    match ext.to_ascii_lowercase().as_str() {
        "mp3" | "mp2" | "mp1" => "MPEG",
        "flac" => "FLAC",
        "ogg" | "oga" => "Vorbis",
        "opus" => "Opus",
        "spx" => "Speex",
        "m4a" | "m4b" => "AAC / ALAC",
        "mp4" | "m4v" => "MP4",
        "aac" => "AAC",
        "wav" | "wave" => "PCM",
        "aif" | "aiff" | "aifc" => "PCM",
        "wv" => "WavPack",
        "ape" => "Monkey's Audio",
        "mpc" | "mpp" => "Musepack",
        "tta" => "TrueAudio",
        "tak" => "TAK",
        "dsf" | "dff" => "DSD",
        "wma" | "wmv" | "asf" => "Windows Media Audio",
        "ac3" => "AC3",
        "dts" => "DTS",
        other => return other.to_ascii_uppercase(),
    }
    .to_string()
}

fn codec_from_matroska(codec: &str) -> String {
    match codec {
        "A_AAC" => "AAC",
        "A_AC3" => "AC3",
        "A_ALAC" => "ALAC",
        "A_DTS" => "DTS",
        "A_EAC3" => "E-AC3",
        "A_FLAC" => "FLAC",
        "A_MLP" => "MLP",
        "A_MPEG/L1" => "MPEG Layer 1",
        "A_MPEG/L2" => "MPEG Layer 2",
        "A_MPEG/L3" => "MPEG Layer 3",
        "A_OPUS" => "Opus",
        "A_PCM/FLOAT/IEEE" => "PCM Float",
        "A_PCM/INT/LIT" => "PCM",
        "A_TRUEHD" => "TrueHD",
        "A_TTA1" => "TrueAudio",
        "A_VORBIS" => "Vorbis",
        "A_WAVPACK4" => "WavPack",
        other => return other.to_string(),
    }
    .to_string()
}

fn format_seconds(seconds: f64) -> String {
    if !seconds.is_finite() || seconds <= 0.0 {
        return String::new();
    }
    let total = seconds.round() as u64;
    let (hours, minutes, secs) = (total / 3600, (total % 3600) / 60, total % 60);
    if hours > 0 {
        format!("{hours:02}:{minutes:02}:{secs:02}")
    } else {
        format!("{minutes:02}:{secs:02}")
    }
}

pub fn default_columns() -> Vec<(String, String)> {
    [
        ("Filename", "__filename"),
        ("Artist", "artist"),
        ("Title", "title"),
        ("Album", "album"),
        ("Track", "track"),
        ("Length", "__length"),
        ("Year", "year"),
        ("Bitrate", "__bitrate"),
        ("Genre", "genre"),
        ("Comment", "comment"),
        ("Dirpath", "__dirpath"),
        ("Codec", "__codec"),
        ("File Type", "__filetype"),
        ("Extension", "__ext"),
    ]
    .iter()
    .map(|(title, field)| (title.to_string(), field.to_string()))
    .collect()
}

pub fn sniff_image_mime(data: &[u8]) -> String {
    if data.starts_with(&[0x89, b'P', b'N', b'G']) {
        "image/png".to_string()
    } else if data.starts_with(&[0xff, 0xd8, 0xff]) {
        "image/jpeg".to_string()
    } else if data.starts_with(b"GIF8") {
        "image/gif".to_string()
    } else if data.starts_with(b"RIFF") && data.get(8..12) == Some(b"WEBP") {
        "image/webp".to_string()
    } else if data.starts_with(b"BM") {
        "image/bmp".to_string()
    } else if data.starts_with(b"II*\0") || data.starts_with(b"MM\0*") {
        "image/tiff".to_string()
    } else {
        String::new()
    }
}
