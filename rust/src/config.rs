//! Configuration compatible with the original application.
//!
//! The original app stores JSON in `~/.config/puddletag/puddletag.conf`.
//! Keeping the same file means an existing install keeps its theme, column
//! layout and preferences when it switches to the Rust front end.

use anyhow::{Context, Result};
use serde_json::{json, Map, Value};
use std::fs;
use std::path::{Path, PathBuf};

pub fn config_dir() -> PathBuf {
    if let Some(dir) = std::env::var_os("XDG_CONFIG_HOME") {
        PathBuf::from(dir).join("puddletag")
    } else if let Some(home) = std::env::var_os("HOME") {
        PathBuf::from(home).join(".config/puddletag")
    } else {
        PathBuf::from(".config/puddletag")
    }
}

#[allow(dead_code)]
pub fn data_dir() -> PathBuf {
    if let Some(dir) = std::env::var_os("XDG_DATA_HOME") {
        PathBuf::from(dir).join("puddletag")
    } else if let Some(home) = std::env::var_os("HOME") {
        PathBuf::from(home).join(".local/share/puddletag")
    } else {
        PathBuf::from(".local/share/puddletag")
    }
}

pub fn config_path() -> PathBuf {
    config_dir().join("puddletag.conf")
}

#[allow(dead_code)]
pub fn actions_dir() -> PathBuf {
    data_dir().join("actions")
}

#[allow(dead_code)]
pub fn log_path() -> PathBuf {
    config_dir().join("puddletag.log")
}

#[derive(Clone, Debug, Default)]
pub struct Config {
    root: Map<String, Value>,
    path: PathBuf,
}

#[allow(dead_code)]
impl Config {
    pub fn load() -> Self {
        let path = config_path();
        let root = fs::read_to_string(&path)
            .ok()
            .and_then(|text| serde_json::from_str::<Value>(&text).ok())
            .and_then(|value| value.as_object().cloned())
            .unwrap_or_default();
        Self { root, path }
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn reload(&mut self) {
        *self = Self::load();
    }

    pub fn save(&self) -> Result<()> {
        if let Some(parent) = self.path.parent() {
            fs::create_dir_all(parent)?;
        }
        let text = serde_json::to_string_pretty(&Value::Object(self.root.clone()))?;
        fs::write(&self.path, text).with_context(|| format!("writing {}", self.path.display()))
    }

    pub fn section(&self, section: &str) -> Option<&Map<String, Value>> {
        self.root.get(section).and_then(Value::as_object)
    }

    pub fn string(&self, section: &str, key: &str, default: &str) -> String {
        self.value(section, key)
            .and_then(Value::as_str)
            .map(str::to_string)
            .unwrap_or_else(|| default.to_string())
    }

    pub fn bool(&self, section: &str, key: &str, default: bool) -> bool {
        match self.value(section, key) {
            Some(Value::Bool(value)) => *value,
            Some(Value::String(value)) => value == "True" || value == "true",
            _ => default,
        }
    }

    pub fn int(&self, section: &str, key: &str, default: i64) -> i64 {
        self.value(section, key)
            .and_then(Value::as_i64)
            .unwrap_or(default)
    }

    pub fn rgb(&self, section: &str, key: &str, default: [u8; 3]) -> [u8; 3] {
        let Some(values) = self.value(section, key).and_then(Value::as_array) else {
            return default;
        };
        if values.len() != 3 {
            return default;
        }
        let mut out = default;
        for (index, value) in values.iter().enumerate() {
            out[index] = value.as_u64().unwrap_or(default[index] as u64).min(255) as u8;
        }
        out
    }

    pub fn string_list(&self, section: &str, key: &str, default: &[&str]) -> Vec<String> {
        match self.value(section, key) {
            Some(Value::Array(values)) => values
                .iter()
                .filter_map(Value::as_str)
                .map(str::to_string)
                .collect(),
            Some(Value::String(value)) => vec![value.clone()],
            _ => default.iter().map(|value| value.to_string()).collect(),
        }
    }

    pub fn int_list(&self, section: &str, key: &str, default: &[i64]) -> Vec<i64> {
        match self.value(section, key) {
            Some(Value::Array(values)) => values.iter().filter_map(Value::as_i64).collect(),
            _ => default.to_vec(),
        }
    }

    pub fn value(&self, section: &str, key: &str) -> Option<&Value> {
        self.section(section).and_then(|section| section.get(key))
    }

    pub fn set(&mut self, section: &str, key: &str, value: Value) {
        let entry = self
            .root
            .entry(section.to_string())
            .or_insert_with(|| Value::Object(Map::new()));
        if !entry.is_object() {
            *entry = Value::Object(Map::new());
        }
        if let Some(section) = entry.as_object_mut() {
            section.insert(key.to_string(), value);
        }
    }

    pub fn set_string(&mut self, section: &str, key: &str, value: impl Into<String>) {
        self.set(section, key, Value::String(value.into()));
    }

    pub fn set_bool(&mut self, section: &str, key: &str, value: bool) {
        self.set(section, key, Value::Bool(value));
    }

    pub fn set_int(&mut self, section: &str, key: &str, value: i64) {
        self.set(section, key, json!(value));
    }

    pub fn set_rgb(&mut self, section: &str, key: &str, value: [u8; 3]) {
        self.set(section, key, json!([value[0], value[1], value[2]]));
    }

    pub fn set_string_list(&mut self, section: &str, key: &str, values: &[String]) {
        self.set(section, key, json!(values));
    }

    // -- common typed settings -------------------------------------------
    pub fn theme(&self) -> String {
        self.string("appearance", "theme", "system")
    }

    pub fn set_theme(&mut self, value: &str) -> Result<()> {
        let value = match value.to_ascii_lowercase().as_str() {
            "light" => "light",
            "dark" => "dark",
            _ => "system",
        };
        self.set_string("appearance", "theme", value);
        self.save()
    }

    pub fn selected_color(&self, default: [u8; 3]) -> [u8; 3] {
        self.rgb("table", "selected_color", default)
    }

    pub fn preview_color(&self, default: [u8; 3]) -> [u8; 3] {
        self.rgb("table", "preview_color", default)
    }

    pub fn cover_pattern(&self) -> String {
        self.string("tags", "cover_pattern", "folder")
    }

    pub fn load_last_folder(&self) -> bool {
        self.general_value("&Load last folder at startup", false)
    }

    pub fn subfolders(&self) -> bool {
        self.general_value("Su&bfolders", true)
    }

    pub fn show_gridlines(&self) -> bool {
        self.general_value("Show &gridlines", true)
    }

    pub fn show_tooltips(&self) -> bool {
        self.general_value("Show tooltips in file-view:", true)
    }

    pub fn show_row_numbers(&self) -> bool {
        self.general_value("Show &row numbers", true)
    }

    pub fn autoresize_columns(&self) -> bool {
        self.general_value("Automatically resize columns to contents", false)
    }

    pub fn preserve_mtime(&self) -> bool {
        self.general_value("&Preserve file modification times (if supported)", false)
    }

    pub fn play_program(&self) -> String {
        self.general_value_str("Program to &play files with:", "xdg-open")
    }

    fn general_value(&self, label: &str, default: bool) -> bool {
        let path = config_dir().join("gensettings");
        let Ok(text) = fs::read_to_string(path) else {
            return default;
        };
        let Ok(value) = serde_json::from_str::<Value>(&text) else {
            return default;
        };
        value
            .get(label)
            .and_then(|entry| entry.get("value"))
            .and_then(Value::as_bool)
            .unwrap_or(default)
    }

    fn general_value_str(&self, label: &str, default: &str) -> String {
        let path = config_dir().join("gensettings");
        let Ok(text) = fs::read_to_string(path) else {
            return default.to_string();
        };
        let Ok(value) = serde_json::from_str::<Value>(&text) else {
            return default.to_string();
        };
        value
            .get(label)
            .and_then(|entry| entry.get("value"))
            .and_then(Value::as_str)
            .map(str::to_string)
            .unwrap_or_else(|| default.to_string())
    }

    pub fn set_general(&mut self, label: &str, value: Value) -> Result<()> {
        let path = config_dir().join("gensettings");
        let mut root = fs::read_to_string(&path)
            .ok()
            .and_then(|text| serde_json::from_str::<Value>(&text).ok())
            .and_then(|value| value.as_object().cloned())
            .unwrap_or_default();
        root.insert(label.to_string(), json!({ "value": value }));
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        fs::write(&path, serde_json::to_string_pretty(&Value::Object(root))?)?;
        Ok(())
    }

    pub fn last_folder(&self) -> Option<PathBuf> {
        let value = self.string("main", "lastfolder", "");
        if value.is_empty() {
            None
        } else {
            Some(PathBuf::from(value))
        }
    }

    pub fn set_last_folder(&mut self, path: &Path) -> Result<()> {
        self.set_string("main", "lastfolder", path.display().to_string());
        self.save()
    }

    pub fn maximized(&self) -> bool {
        self.bool("main", "maximized", true)
    }

    pub fn set_maximized(&mut self, value: bool) -> Result<()> {
        self.set_bool("main", "maximized", value);
        self.save()
    }

    pub fn patterns(&self) -> Vec<String> {
        self.string_list(
            "editor",
            "patterns",
            &[
                "%artist% - $num(%track%,2) - %title%",
                "%artist% - %album% - $num(%track%,2) - %title%",
                "%artist% - %title%",
                "%artist% - %album%",
                "%artist% - Track %track%",
                "%artist% - %title%",
            ],
        )
    }

    pub fn set_patterns(&mut self, patterns: &[String]) -> Result<()> {
        self.set_string_list("editor", "patterns", patterns);
        self.save()
    }

    pub fn pattern_index(&self) -> usize {
        self.int("editor", "index", 0).max(0) as usize
    }

    pub fn set_pattern_index(&mut self, index: usize) -> Result<()> {
        self.set_int("editor", "index", index as i64);
        self.save()
    }
}
