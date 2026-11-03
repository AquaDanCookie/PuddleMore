//! Minimal safe wrapper around the TagLib C API.
//!
//! TagLib's property map is important here: unlike Lofty's fixed `ItemKey`
//! enum it preserves arbitrary fields (Vorbis comments, ASF attributes, MP4
//! freeform atoms) instead of silently dropping them. That is what lets the
//! editor expose *all* metadata, not just the commonly mapped subset.

use crate::metadata::ImageData;
use anyhow::{anyhow, Result};
use std::collections::{BTreeMap, HashMap};
use std::ffi::{c_char, c_int, c_uint, CStr, CString};
use std::path::Path;
use std::slice;
use std::sync::Once;

#[repr(C)]
struct TagLibFile {
    _private: [u8; 0],
}

#[repr(C)]
struct TagLibAudioProperties {
    _private: [u8; 0],
}

#[link(name = "tag_c")]
unsafe extern "C" {
    fn taglib_file_new(filename: *const c_char) -> *mut TagLibFile;
    fn taglib_file_free(file: *mut TagLibFile);
    fn taglib_file_is_valid(file: *const TagLibFile) -> c_int;
    fn taglib_file_save(file: *mut TagLibFile) -> c_int;
    fn taglib_file_audioproperties(file: *const TagLibFile) -> *const TagLibAudioProperties;
    fn taglib_audioproperties_length(properties: *const TagLibAudioProperties) -> c_int;
    fn taglib_audioproperties_bitrate(properties: *const TagLibAudioProperties) -> c_int;
    fn taglib_audioproperties_samplerate(properties: *const TagLibAudioProperties) -> c_int;
    fn taglib_audioproperties_channels(properties: *const TagLibAudioProperties) -> c_int;
    fn taglib_property_keys(file: *const TagLibFile) -> *mut *mut c_char;
    fn taglib_property_get(file: *const TagLibFile, property: *const c_char) -> *mut *mut c_char;
    fn taglib_property_set(file: *mut TagLibFile, property: *const c_char, value: *const c_char);
    fn taglib_property_set_append(
        file: *mut TagLibFile,
        property: *const c_char,
        value: *const c_char,
    );
    fn taglib_property_free(properties: *mut *mut c_char);
    fn taglib_id3v2_set_default_text_encoding(encoding: c_int);
    fn taglib_complex_property_get(
        file: *const TagLibFile,
        key: *const c_char,
    ) -> *mut *mut *mut TagLibComplexAttribute;
    fn taglib_complex_property_free(props: *mut *mut *mut TagLibComplexAttribute);
    fn taglib_complex_property_set(
        file: *mut TagLibFile,
        key: *const c_char,
        value: *const *const TagLibComplexAttribute,
    ) -> c_int;
    fn taglib_complex_property_set_append(
        file: *mut TagLibFile,
        key: *const c_char,
        value: *const *const TagLibComplexAttribute,
    ) -> c_int;
}

#[repr(C)]
pub union TagLibVariantValue {
    pub string_value: *mut c_char,
    pub byte_vector_value: *mut c_char,
    pub bool_value: c_int,
    pub int_value: c_int,
    pub uint_value: c_uint,
    pub longlong_value: i64,
    pub ulonglong_value: u64,
    pub double_value: f64,
}

#[repr(C)]
pub struct TagLibVariant {
    pub type_: c_int,
    pub size: c_uint,
    pub value: TagLibVariantValue,
}

#[repr(C)]
pub struct TagLibComplexAttribute {
    pub key: *mut c_char,
    pub value: TagLibVariant,
}

pub struct ReadResult {
    pub fields: BTreeMap<String, Vec<String>>,
    /// Canonical field name -> original TagLib property name.
    pub property_names: HashMap<String, String>,
    pub length_seconds: f64,
    pub bitrate: u32,
    pub samplerate: u32,
    pub channels: u32,
    pub pictures: Vec<ImageData>,
}

/// TagLib's C API defaults to Latin-1 for ID3v2. Use UTF-8 so non-Latin
/// metadata survives a write.
pub fn configure() {
    static ONCE: Once = Once::new();
    ONCE.call_once(|| unsafe {
        // TagLib_ID3v2_UTF8 is the fourth value in the enum.
        taglib_id3v2_set_default_text_encoding(3);
    });
}

pub fn read(path: &Path) -> Result<ReadResult> {
    configure();
    let path = c_path(path)?;
    let raw = unsafe { taglib_file_new(path.as_ptr()) };
    if raw.is_null() {
        return Err(anyhow!("TagLib does not recognise this file"));
    }
    let file = FileGuard(raw);
    if unsafe { taglib_file_is_valid(file.0) } == 0 {
        return Err(anyhow!("TagLib could not parse this file"));
    }

    let mut fields: BTreeMap<String, Vec<String>> = BTreeMap::new();
    let mut property_names = HashMap::new();
    for property in property_keys(file.0) {
        let Some(field) = field_for_property(&property) else {
            continue;
        };
        let values = property_values(file.0, &property);
        if values.is_empty() {
            continue;
        }
        store_values(&mut fields, &mut property_names, &field, &property, values);
    }

    let mut length_seconds = 0.0;
    let mut bitrate = 0;
    let mut samplerate = 0;
    let mut channels = 0;
    let properties = unsafe { taglib_file_audioproperties(file.0) };
    if !properties.is_null() {
        length_seconds = unsafe { taglib_audioproperties_length(properties) }.max(0) as f64;
        bitrate = unsafe { taglib_audioproperties_bitrate(properties) }.max(0) as u32;
        samplerate = unsafe { taglib_audioproperties_samplerate(properties) }.max(0) as u32;
        channels = unsafe { taglib_audioproperties_channels(properties) }.max(0) as u32;
    }

    let pictures = read_pictures(file.0);
    Ok(ReadResult {
        fields,
        property_names,
        length_seconds,
        bitrate,
        samplerate,
        channels,
        pictures,
    })
}

fn store_values(
    fields: &mut BTreeMap<String, Vec<String>>,
    property_names: &mut HashMap<String, String>,
    field: &str,
    property: &str,
    values: Vec<String>,
) {
    property_names
        .entry(field.to_string())
        .or_insert_with(|| property.to_string());

    // TrackLib hands back "2/8" for formats that keep the total next to the
    // number. PuddleMore exposes the two halves as separate fields.
    if matches!(field, "track" | "disc") {
        if values.len() == 1 {
            if let Some((number, total)) = values[0].split_once('/') {
                let total_field = if field == "track" {
                    "totaltracks"
                } else {
                    "totaldiscs"
                };
                fields
                    .entry(field.to_string())
                    .or_default()
                    .push(number.trim().to_string());
                fields
                    .entry(total_field.to_string())
                    .or_default()
                    .push(total.trim().to_string());
                property_names
                    .entry(total_field.to_string())
                    .or_insert_with(|| property.to_string());
                return;
            }
        }
    }
    fields.entry(field.to_string()).or_default().extend(values);
}

pub fn write(
    path: &Path,
    fields: &BTreeMap<String, Vec<String>>,
    property_names: &HashMap<String, String>,
    images: Option<&[ImageData]>,
) -> Result<()> {
    configure();
    let path_c = c_path(path)?;
    let raw = unsafe { taglib_file_new(path_c.as_ptr()) };
    if raw.is_null() {
        return Err(anyhow!("TagLib does not recognise this file"));
    }
    let file = FileGuard(raw);
    if let Some(images) = images {
        write_pictures(file.0, images);
    }

    let (desired, managed) = desired_properties(fields, property_names);
    for property in property_keys(file.0) {
        if managed.contains(&property) && !desired.contains_key(&property) {
            let property_c = CString::new(property.as_str())?;
            unsafe { taglib_property_set(file.0, property_c.as_ptr(), std::ptr::null()) };
        }
    }
    for (property, values) in &desired {
        let property_c = CString::new(property.as_str())?;
        let mut values = values.iter().filter(|value| !value.is_empty());
        if let Some(first) = values.next() {
            let value_c = CString::new(first.as_str())?;
            unsafe { taglib_property_set(file.0, property_c.as_ptr(), value_c.as_ptr()) };
        }
        for value in values {
            let value_c = CString::new(value.as_str())?;
            unsafe {
                taglib_property_set_append(file.0, property_c.as_ptr(), value_c.as_ptr());
            }
        }
    }

    if unsafe { taglib_file_save(file.0) } == 0 {
        return Err(anyhow!("TagLib could not save this file"));
    }
    Ok(())
}

fn read_pictures(file: *const TagLibFile) -> Vec<ImageData> {
    let key = CString::new("PICTURE").expect("static key");
    let props = unsafe { taglib_complex_property_get(file, key.as_ptr()) };
    if props.is_null() {
        return Vec::new();
    }
    let mut images = Vec::new();
    let mut index = 0usize;
    loop {
        let attributes = unsafe { *props.add(index) };
        if attributes.is_null() {
            break;
        }
        let mut data = None;
        let mut mime = String::new();
        let mut description = String::new();
        let mut picture_type = String::new();
        let mut attr_index = 0usize;
        loop {
            let attribute = unsafe { *attributes.add(attr_index) };
            if attribute.is_null() {
                break;
            }
            let key = unsafe { CStr::from_ptr((*attribute).key) }
                .to_string_lossy()
                .to_string();
            match key.as_str() {
                "data" => {
                    let value = unsafe { &(*attribute).value };
                    if value.type_ == 9 {
                        let pointer = unsafe { value.value.byte_vector_value };
                        if !pointer.is_null() {
                            let bytes = unsafe {
                                slice::from_raw_parts(pointer as *const u8, value.size as usize)
                            };
                            data = Some(bytes.to_vec());
                        }
                    }
                }
                "mimeType" => {
                    let value = unsafe { &(*attribute).value };
                    if value.type_ == 7 {
                        let pointer = unsafe { value.value.string_value };
                        if !pointer.is_null() {
                            mime = unsafe { CStr::from_ptr(pointer) }
                                .to_string_lossy()
                                .to_string();
                        }
                    }
                }
                "description" => {
                    let value = unsafe { &(*attribute).value };
                    if value.type_ == 7 {
                        let pointer = unsafe { value.value.string_value };
                        if !pointer.is_null() {
                            description = unsafe { CStr::from_ptr(pointer) }
                                .to_string_lossy()
                                .to_string();
                        }
                    }
                }
                "pictureType" => {
                    let value = unsafe { &(*attribute).value };
                    if value.type_ == 7 {
                        let pointer = unsafe { value.value.string_value };
                        if !pointer.is_null() {
                            picture_type = unsafe { CStr::from_ptr(pointer) }
                                .to_string_lossy()
                                .to_string();
                        }
                    }
                }
                _ => {}
            }
            attr_index += 1;
        }
        if let Some(data) = data {
            let mime = if mime.is_empty() {
                crate::metadata::sniff_image_mime(&data)
            } else {
                mime
            };
            images.push(ImageData {
                data,
                mime,
                description,
                image_type: type_from_name(&picture_type),
            });
        }
        index += 1;
    }
    unsafe { taglib_complex_property_free(props) };
    images
}

fn type_from_name(name: &str) -> u8 {
    let names = [
        "Other",
        "File Icon",
        "Other File Icon",
        "Front Cover",
        "Back Cover",
        "Leaflet Page",
        "Media",
        "Lead Artist",
        "Artist",
        "Conductor",
        "Band",
        "Composer",
        "Lyricist",
        "Recording Location",
        "During Recording",
        "During Performance",
        "Movie Screen Capture",
        "Coloured Fish",
        "Illustration",
        "Band Logo",
        "Publisher Logo",
    ];
    names
        .iter()
        .position(|candidate| candidate.eq_ignore_ascii_case(name))
        .map(|index| index as u8)
        .unwrap_or(3)
}

fn type_name(image_type: u8) -> &'static str {
    let names = [
        "Other",
        "File Icon",
        "Other File Icon",
        "Front Cover",
        "Back Cover",
        "Leaflet Page",
        "Media",
        "Lead Artist",
        "Artist",
        "Conductor",
        "Band",
        "Composer",
        "Lyricist",
        "Recording Location",
        "During Recording",
        "During Performance",
        "Movie Screen Capture",
        "Coloured Fish",
        "Illustration",
        "Band Logo",
        "Publisher Logo",
    ];
    names
        .get(image_type as usize)
        .copied()
        .unwrap_or("Front Cover")
}

fn write_pictures(file: *mut TagLibFile, images: &[ImageData]) {
    let key = CString::new("PICTURE").expect("static key");
    unsafe { taglib_complex_property_set(file, key.as_ptr(), std::ptr::null()) };
    for image in images {
        let mime = CString::new(image.mime.as_str()).unwrap_or_default();
        let description = CString::new(image.description.as_str()).unwrap_or_default();
        let picture_type = CString::new(type_name(image.image_type)).unwrap_or_default();
        let data_key = CString::new("data").expect("static key");
        let mime_key = CString::new("mimeType").expect("static key");
        let description_key = CString::new("description").expect("static key");
        let type_key = CString::new("pictureType").expect("static key");
        let attributes = [
            TagLibComplexAttribute {
                key: data_key.as_ptr() as *mut c_char,
                value: TagLibVariant {
                    type_: 9,
                    size: image.data.len() as c_uint,
                    value: TagLibVariantValue {
                        byte_vector_value: image.data.as_ptr() as *mut c_char,
                    },
                },
            },
            TagLibComplexAttribute {
                key: mime_key.as_ptr() as *mut c_char,
                value: TagLibVariant {
                    type_: 7,
                    size: 0,
                    value: TagLibVariantValue {
                        string_value: mime.as_ptr() as *mut c_char,
                    },
                },
            },
            TagLibComplexAttribute {
                key: description_key.as_ptr() as *mut c_char,
                value: TagLibVariant {
                    type_: 7,
                    size: 0,
                    value: TagLibVariantValue {
                        string_value: description.as_ptr() as *mut c_char,
                    },
                },
            },
            TagLibComplexAttribute {
                key: type_key.as_ptr() as *mut c_char,
                value: TagLibVariant {
                    type_: 7,
                    size: 0,
                    value: TagLibVariantValue {
                        string_value: picture_type.as_ptr() as *mut c_char,
                    },
                },
            },
        ];
        let mut pointers: Vec<*const TagLibComplexAttribute> = attributes
            .iter()
            .map(|attribute| attribute as *const _)
            .collect();
        pointers.push(std::ptr::null());
        unsafe {
            taglib_complex_property_set_append(file, key.as_ptr(), pointers.as_ptr());
        }
    }
}

fn desired_properties(
    fields: &BTreeMap<String, Vec<String>>,
    property_names: &HashMap<String, String>,
) -> (
    BTreeMap<String, Vec<String>>,
    std::collections::BTreeSet<String>,
) {
    let mut desired: BTreeMap<String, Vec<String>> = BTreeMap::new();
    let mut managed: std::collections::BTreeSet<String> =
        property_names.values().cloned().collect();

    for (field, values) in fields {
        if field.starts_with("__")
            || matches!(
                field.as_str(),
                "track" | "totaltracks" | "disc" | "totaldiscs"
            )
        {
            continue;
        }
        let values: Vec<String> = values
            .iter()
            .filter(|value| !value.is_empty())
            .cloned()
            .collect();
        if values.is_empty() {
            continue;
        }
        let property = property_names
            .get(field)
            .cloned()
            .unwrap_or_else(|| property_for_field(field));
        managed.insert(property.clone());
        desired.insert(property, values);
    }

    for (number_field, total_field, property) in [
        ("track", "totaltracks", "TRACKNUMBER"),
        ("disc", "totaldiscs", "DISCNUMBER"),
    ] {
        let number = fields
            .get(number_field)
            .and_then(|values| values.iter().find(|value| !value.is_empty()));
        let total = fields
            .get(total_field)
            .and_then(|values| values.iter().find(|value| !value.is_empty()));
        if number.is_some() || total.is_some() {
            let value = match (number, total) {
                (Some(number), Some(total)) => format!("{number}/{total}"),
                (Some(number), None) => number.clone(),
                (None, Some(total)) => format!("0/{total}"),
                (None, None) => unreachable!(),
            };
            managed.insert(property.to_string());
            desired.insert(property.to_string(), vec![value]);
        }
    }

    (desired, managed)
}

fn field_for_property(property: &str) -> Option<String> {
    let field = match property.to_ascii_uppercase().as_str() {
        "TITLE" => "title",
        "ARTIST" => "artist",
        "ALBUM" => "album",
        "ALBUMARTIST" | "ALBUM_ARTIST" => "albumartist",
        "COMPOSER" => "composer",
        "CONDUCTOR" => "conductor",
        "COMMENT" => "comment",
        "DESCRIPTION" => "description",
        "GENRE" => "genre",
        "DATE" | "YEAR" => "year",
        "TRACKNUMBER" | "TRACK" => "track",
        "TRACKTOTAL" | "TOTALTRACKS" => "totaltracks",
        "DISCNUMBER" | "DISC" => "disc",
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
        "PICTURE" | "COVERART" | "COVERARTMIME" | "CUESHEET" => return None,
        other => return Some(other.to_ascii_lowercase()),
    };
    Some(field.to_string())
}

fn property_for_field(field: &str) -> String {
    match field.to_ascii_lowercase().as_str() {
        "title" => "TITLE",
        "artist" => "ARTIST",
        "album" => "ALBUM",
        "albumartist" => "ALBUMARTIST",
        "composer" => "COMPOSER",
        "conductor" => "CONDUCTOR",
        "comment" => "COMMENT",
        "description" => "DESCRIPTION",
        "genre" => "GENRE",
        "year" => "DATE",
        "track" => "TRACKNUMBER",
        "totaltracks" => "TRACKNUMBER",
        "disc" => "DISCNUMBER",
        "totaldiscs" => "DISCNUMBER",
        "bpm" => "BPM",
        "isrc" => "ISRC",
        "language" => "LANGUAGE",
        "lyrics" => "LYRICS",
        "copyright" => "COPYRIGHT",
        "encodedby" => "ENCODEDBY",
        "encodersettings" => "ENCODING",
        "encoder" => "ENCODER",
        "organization" => "LABEL",
        "catalognumber" => "CATALOGNUMBER",
        "mood" => "MOOD",
        "initialkey" => "INITIALKEY",
        "subtitle" => "SUBTITLE",
        "grouping" => "GROUPING",
        "work" => "WORK",
        "movementname" => "MOVEMENTNAME",
        "movementnumber" => "MOVEMENTNUMBER",
        "movementtotal" => "MOVEMENTTOTAL",
        "originalyear" => "ORIGINALDATE",
        "originalalbum" => "ORIGINALALBUM",
        "originalartist" => "ORIGINALARTIST",
        "releasedate" => "RELEASEDATE",
        "lyricist" => "LYRICIST",
        "arranger" => "ARRANGER",
        other => return other.to_ascii_uppercase(),
    }
    .to_string()
}

fn property_keys(file: *const TagLibFile) -> Vec<String> {
    let pointers = unsafe { taglib_property_keys(file) };
    if pointers.is_null() {
        return Vec::new();
    }
    let mut keys = Vec::new();
    let mut index = 0usize;
    loop {
        let pointer = unsafe { *pointers.add(index) };
        if pointer.is_null() {
            break;
        }
        keys.push(
            unsafe { CStr::from_ptr(pointer) }
                .to_string_lossy()
                .into_owned(),
        );
        index += 1;
    }
    unsafe { taglib_property_free(pointers) };
    keys
}

fn property_values(file: *const TagLibFile, property: &str) -> Vec<String> {
    let Ok(property) = CString::new(property) else {
        return Vec::new();
    };
    let pointers = unsafe { taglib_property_get(file, property.as_ptr()) };
    if pointers.is_null() {
        return Vec::new();
    }
    let mut values = Vec::new();
    let mut index = 0usize;
    loop {
        let pointer = unsafe { *pointers.add(index) };
        if pointer.is_null() {
            break;
        }
        values.push(
            unsafe { CStr::from_ptr(pointer) }
                .to_string_lossy()
                .into_owned(),
        );
        index += 1;
    }
    unsafe { taglib_property_free(pointers) };
    values
}

fn c_path(path: &Path) -> Result<CString> {
    let value = path
        .to_str()
        .ok_or_else(|| anyhow!("path is not valid UTF-8: {}", path.display()))?;
    Ok(CString::new(value)?)
}

struct FileGuard(*mut TagLibFile);

impl Drop for FileGuard {
    fn drop(&mut self) {
        unsafe { taglib_file_free(self.0) };
    }
}
