//! Tag export templates (`$loop`, `%field%`, `$function`).

use crate::metadata::Track;
use crate::pattern;
use anyhow::{Context, Result};
use std::fs;
use std::path::{Path, PathBuf};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Preset {
    Csv,
    Html,
    Rtf,
    Txt,
    M3u,
}

impl Preset {
    pub fn label(self) -> &'static str {
        match self {
            Self::Csv => "CSV",
            Self::Html => "HTML",
            Self::Rtf => "RTF",
            Self::Txt => "TXT",
            Self::M3u => "M3U",
        }
    }

    pub fn template(self) -> &'static str {
        match self {
            Self::Csv => {
                "$loop()%_filename%,%artist%,%album%,%title%,%track%,%year%,%genre%$CRLF()$loopend()"
            }
            Self::Html => {
                "<table><tr><th>#</th><th>Filename</th><th>Artist</th><th>Album</th><th>Title</th><th>Track</th><th>Year</th></tr>$loop()<tr><td>%_count%</td><td>%_filename%</td><td>%artist%</td><td>%album%</td><td>%title%</td><td>%track%</td><td>%year%</td></tr>$loopend()</table>"
            }
            Self::Rtf => {
                "$loop()%artist% - %album% - %title% - %track% - %year%$CRLF()$loopend()"
            }
            Self::Txt => "%artist% - %title%$LF()",
            Self::M3u => "%_fullpath%$LF()",
        }
    }
}

pub fn export_config_path() -> PathBuf {
    crate::config::config_dir().join("export.ini")
}

pub fn load_template() -> String {
    let path = export_config_path();
    let Ok(text) = fs::read_to_string(path) else {
        return Preset::Csv.template().to_string();
    };
    let mut in_export = false;
    for line in text.lines() {
        let line = line.trim();
        if line.starts_with('[') {
            in_export = line.eq_ignore_ascii_case("[export]");
            continue;
        }
        if in_export {
            if let Some((key, value)) = line.split_once('=') {
                if key.trim().eq_ignore_ascii_case("format") {
                    return value.trim().to_string();
                }
            }
        }
    }
    Preset::Csv.template().to_string()
}

pub fn save_template(template: &str) -> Result<()> {
    let path = export_config_path();
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    fs::write(&path, format!("[export]\nformat={template}\n"))
        .with_context(|| format!("writing {}", path.display()))
}

pub fn render(template: &str, tracks: &[Track]) -> Result<String> {
    let (header, body, footer, limit) = split_loop(template);
    let mut output = String::new();
    let total = tracks.len();
    if let Some(first) = tracks.first() {
        let mut empty = first.clone();
        empty.fields.clear();
        empty
            .fields
            .insert("_files".to_string(), vec![total.to_string()]);
        output.push_str(&render_one(header, &empty, 0, total)?);
    }
    let count = if limit > 0 { limit.min(total) } else { total };
    for (index, track) in tracks.iter().take(count).enumerate() {
        output.push_str(&render_one(body, track, index + 1, total)?);
    }
    if let Some(first) = tracks.first() {
        let mut empty = first.clone();
        empty.fields.clear();
        empty
            .fields
            .insert("_files".to_string(), vec![total.to_string()]);
        output.push_str(&render_one(footer, &empty, 0, total)?);
    }
    Ok(output)
}

fn render_one(template: &str, track: &Track, count: usize, total: usize) -> Result<String> {
    let mut clone = track.clone();
    clone.fields.extend(export_fields(track, count, total));
    pattern::render(template, &clone)
}

fn export_fields(track: &Track, count: usize, total: usize) -> Vec<(String, Vec<String>)> {
    let path = &track.path;
    let filename = path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or_default();
    let stem = path
        .file_stem()
        .and_then(|stem| stem.to_str())
        .unwrap_or_default();
    let ext = path.extension().and_then(|ext| ext.to_str()).unwrap_or("");
    let folder = path
        .parent()
        .map(|parent| parent.display().to_string())
        .unwrap_or_default();
    let parent = path
        .parent()
        .and_then(|parent| parent.file_name())
        .and_then(|name| name.to_str())
        .unwrap_or_default();
    let full = path.display().to_string();
    let base = if ext.is_empty() {
        full.clone()
    } else {
        full.trim_end_matches(&format!(".{ext}")).to_string()
    };
    let mut fields = vec![
        ("_filename".to_string(), vec![stem.to_string()]),
        ("_filenameext".to_string(), vec![filename.to_string()]),
        ("_ext".to_string(), vec![ext.to_string()]),
        ("_folderpath".to_string(), vec![folder]),
        ("_fullpath".to_string(), vec![full.clone()]),
        ("_filepath".to_string(), vec![full]),
        ("_base".to_string(), vec![base]),
        ("_parentdir".to_string(), vec![parent.to_string()]),
        ("_count".to_string(), vec![count.to_string()]),
        ("_files".to_string(), vec![total.to_string()]),
    ];
    fields.retain(|(_, values)| !values.is_empty());
    fields
}

fn split_loop(template: &str) -> (&str, &str, &str, usize) {
    let Some(start) = template.find("$loop(") else {
        return ("", template, "", 0);
    };
    let after = &template[start + 6..];
    let Some(close) = after.find(')') else {
        return ("", template, "", 0);
    };
    let number: usize = after[..close].trim().parse().unwrap_or(0);
    let body_start = start + 6 + close + 1;
    let Some(end_offset) = template[body_start..].find("$loopend()") else {
        return ("", template, "", 0);
    };
    let body_end = body_start + end_offset;
    (
        &template[..start],
        &template[body_start..body_end],
        &template[body_end + "$loopend()".len()..],
        number,
    )
}

pub fn save(path: &Path, contents: &str) -> Result<()> {
    fs::write(path, contents).with_context(|| format!("writing {}", path.display()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::metadata::{Backend, Track};
    use std::collections::BTreeMap;

    fn track() -> Track {
        let mut fields = BTreeMap::new();
        fields.insert("artist".to_string(), vec!["Artist".to_string()]);
        fields.insert("title".to_string(), vec!["Title".to_string()]);
        Track {
            path: PathBuf::from("/music/song.mp3"),
            fields,
            original_fields: BTreeMap::new(),
            dirty: false,
            images: Vec::new(),
            original_images: Vec::new(),
            error: None,
            format: "MP3".to_string(),
            codec: "MPEG".to_string(),
            length: String::new(),
            bitrate: String::new(),
            matroska_names: Default::default(),
            taglib_names: Default::default(),
            ape_names: Default::default(),
            segment_title_present: false,
            segment_title_only: false,
            backend: Backend::TagLib,
        }
    }

    #[test]
    fn renders_csv_and_html() {
        let tracks = vec![track()];
        let csv = render(Preset::Csv.template(), &tracks).unwrap();
        assert!(csv.contains("song,Artist,,Title"));
        let html = render(Preset::Html.template(), &tracks).unwrap();
        assert!(html.contains("<td>Artist</td>"));
    }
}
