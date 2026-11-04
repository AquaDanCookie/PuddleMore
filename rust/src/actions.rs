//! Saved Actions and QuickActions.
//!
//! Action files live in `~/.local/share/puddletag/actions`. Both the current
//! JSON format and the legacy INI form are accepted.

use crate::config;
use crate::metadata::Track;
use crate::pattern;
use anyhow::Result;
use serde_json::Value;
use std::collections::BTreeMap;
use std::fs;
use std::path::Path;

#[derive(Clone, Debug)]
pub struct ActionFunction {
    pub name: String,
    pub fields: Vec<String>,
    pub args: Vec<String>,
}

#[derive(Clone, Debug)]
pub struct Macro {
    pub name: String,
    pub functions: Vec<ActionFunction>,
    pub filename: Option<String>,
}

impl Macro {
    pub fn summary(&self) -> String {
        let names: Vec<String> = self
            .functions
            .iter()
            .map(|function| function.name.clone())
            .collect();
        names.join(" → ")
    }
}

pub fn load_macros() -> Vec<Macro> {
    let dir = config::actions_dir();
    let mut macros = Vec::new();
    let Ok(entries) = fs::read_dir(&dir) else {
        return macros;
    };
    let mut paths: Vec<_> = entries
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .filter(|path| {
            path.extension()
                .and_then(|ext| ext.to_str())
                .is_some_and(|ext| ext.eq_ignore_ascii_case("action"))
        })
        .collect();
    paths.sort();
    for path in paths {
        if let Some(parsed) = load_macro(&path) {
            macros.push(parsed);
        }
    }
    macros
}

fn load_macro(path: &Path) -> Option<Macro> {
    let text = fs::read_to_string(path).ok()?;
    if let Ok(value) = serde_json::from_str::<Value>(&text) {
        return parse_json(value, path);
    }
    parse_ini(&text, path)
}

fn parse_json(value: Value, path: &Path) -> Option<Macro> {
    let name = value
        .get("info")
        .and_then(|info| info.get("name"))
        .and_then(Value::as_str)
        .map(str::to_string)
        .or_else(|| {
            path.file_stem()
                .and_then(|stem| stem.to_str())
                .map(str::to_string)
        })?;
    let mut functions = Vec::new();
    let object = value.as_object()?;
    let mut keys: Vec<&String> = object
        .keys()
        .filter(|key| key.starts_with("Func"))
        .collect();
    keys.sort_by_key(|key| {
        key.trim_start_matches("Func")
            .parse::<usize>()
            .unwrap_or(usize::MAX)
    });
    for key in keys {
        let entry = &object[key];
        let func_name = entry
            .get("func_name")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string();
        if func_name.is_empty() {
            continue;
        }
        let fields = string_list(entry.get("fields"));
        let args = string_list(entry.get("arguments"));
        functions.push(ActionFunction {
            name: func_name,
            fields,
            args,
        });
    }
    Some(Macro {
        name,
        functions,
        filename: Some(path.display().to_string()),
    })
}

fn parse_ini(text: &str, path: &Path) -> Option<Macro> {
    let mut name = path
        .file_stem()
        .and_then(|stem| stem.to_str())
        .unwrap_or("Action")
        .to_string();
    let mut sections: BTreeMap<String, BTreeMap<String, String>> = BTreeMap::new();
    let mut current = String::new();
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with(';') || line.starts_with('#') {
            continue;
        }
        if line.starts_with('[') && line.ends_with(']') {
            current = line[1..line.len() - 1].to_string();
            continue;
        }
        if let Some((key, value)) = line.split_once('=') {
            sections
                .entry(current.clone())
                .or_default()
                .insert(key.trim().to_ascii_lowercase(), value.trim().to_string());
        }
    }
    if let Some(info) = sections.get("info") {
        if let Some(value) = info.get("name") {
            name = value.clone();
        }
    }
    let mut functions = Vec::new();
    let mut keys: Vec<&String> = sections
        .keys()
        .filter(|key| key.starts_with("Func"))
        .collect();
    keys.sort_by_key(|key| {
        key.trim_start_matches("Func")
            .parse::<usize>()
            .unwrap_or(usize::MAX)
    });
    for key in keys {
        let Some(entry) = sections.get(key) else {
            continue;
        };
        let Some(func_name) = entry.get("func_name") else {
            continue;
        };
        let fields = entry
            .get("fields")
            .map(|value| split_list(value))
            .unwrap_or_default();
        let args = entry
            .get("arguments")
            .map(|value| split_list(value))
            .unwrap_or_default();
        functions.push(ActionFunction {
            name: func_name.clone(),
            fields,
            args,
        });
    }
    Some(Macro {
        name,
        functions,
        filename: Some(path.display().to_string()),
    })
}

fn string_list(value: Option<&Value>) -> Vec<String> {
    match value {
        Some(Value::Array(values)) => values
            .iter()
            .filter_map(Value::as_str)
            .map(str::to_string)
            .collect(),
        Some(Value::String(value)) => split_list(value),
        _ => Vec::new(),
    }
}

fn split_list(value: &str) -> Vec<String> {
    value
        .split(',')
        .map(|part| part.trim().trim_matches('"').to_string())
        .filter(|part| !part.is_empty())
        .collect()
}

pub fn apply_macro(
    macro_: &Macro,
    track: &mut Track,
    field_override: Option<&[String]>,
) -> Vec<String> {
    let mut changed = Vec::new();
    for function in &macro_.functions {
        let fields = if let Some(fields) = field_override {
            fields.to_vec()
        } else {
            expand_fields(&function.fields, track)
        };
        let lower = function.name.to_ascii_lowercase();
        if lower == "filenametotag" {
            let pattern = function.args.first().cloned().unwrap_or_default();
            let filename = track.display("__filename");
            let parsed = pattern::filename_to_tag(&pattern, &filename);
            for (field, values) in parsed {
                if let Some(value) = values.first() {
                    track.fields.insert(field.clone(), vec![value.clone()]);
                    changed.push(field);
                }
            }
            continue;
        }
        if lower == "remove_except" {
            let keep: Vec<String> = function
                .args
                .iter()
                .flat_map(|arg| arg.split(';'))
                .map(|value| value.trim().to_ascii_lowercase())
                .collect();
            let remove: Vec<String> = track
                .fields
                .keys()
                .filter(|field| !field.starts_with("__"))
                .filter(|field| !keep.contains(&field.to_ascii_lowercase()))
                .cloned()
                .collect();
            for field in remove {
                track.fields.remove(&field);
                changed.push(field);
            }
            continue;
        }
        for field in fields {
            let value = track.display(&field);
            let result =
                pattern::apply_field_function(&function.name, &function.args, &value, track);
            match result {
                Ok(result) => {
                    if result.is_empty() {
                        track.fields.remove(&field);
                    } else {
                        track.fields.insert(field.clone(), vec![result]);
                    }
                    if !changed.contains(&field) {
                        changed.push(field);
                    }
                }
                Err(err) => {
                    track.error = Some(format!("{err:#}"));
                    return changed;
                }
            }
        }
    }
    track.dirty = track.fields != track.original_fields;
    changed
}

fn expand_fields(fields: &[String], track: &Track) -> Vec<String> {
    let mut out = Vec::new();
    for field in fields {
        match field.as_str() {
            "__all" => out.extend(
                track
                    .fields
                    .keys()
                    .filter(|field| !field.starts_with("__"))
                    .cloned(),
            ),
            field if field.starts_with('~') => {
                let excluded: Vec<String> = field[1..]
                    .split(',')
                    .map(|value| value.trim().to_ascii_lowercase())
                    .collect();
                out.extend(
                    track
                        .fields
                        .keys()
                        .filter(|key| {
                            !key.starts_with("__") && !excluded.contains(&key.to_ascii_lowercase())
                        })
                        .cloned(),
                );
            }
            field => out.push(field.to_string()),
        }
    }
    out.sort();
    out.dedup();
    out
}

pub fn function_names() -> &'static [&'static str] {
    &[
        "format",
        "titleCase",
        "upper",
        "lower",
        "replace",
        "strip",
        "num",
        "remove_fields",
        "remove_except",
        "filenametotag",
        "left",
        "right",
        "mid",
        "len",
        "to_num",
    ]
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::metadata::{Backend, Track};
    use std::collections::BTreeMap;
    use std::path::PathBuf;

    fn track() -> Track {
        let mut fields = BTreeMap::new();
        fields.insert("artist".to_string(), vec!["artist".to_string()]);
        fields.insert("title".to_string(), vec!["title".to_string()]);
        Track {
            path: PathBuf::from("/tmp/test.mp3"),
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
    fn applies_format_action() {
        let macro_ = Macro {
            name: "Format".to_string(),
            functions: vec![ActionFunction {
                name: "format".to_string(),
                fields: vec!["title".to_string()],
                args: vec!["%artist% - %title%".to_string()],
            }],
            filename: None,
        };
        let mut track = track();
        apply_macro(&macro_, &mut track, None);
        assert_eq!(track.display("title"), "artist - title");
    }

    #[test]
    fn expands_all_fields() {
        let macro_ = Macro {
            name: "Upper".to_string(),
            functions: vec![ActionFunction {
                name: "upper".to_string(),
                fields: vec!["__all".to_string()],
                args: vec![],
            }],
            filename: None,
        };
        let mut track = track();
        apply_macro(&macro_, &mut track, None);
        assert_eq!(track.display("artist"), "ARTIST");
        assert_eq!(track.display("title"), "TITLE");
    }
}
