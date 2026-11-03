//! Puddletag-style `%field%` / `$function(...)` patterns.
//!
//! This is the shared evaluator behind Convert, Actions and the pattern combo.

use crate::metadata::Track;
use anyhow::{anyhow, Result};
use regex::Regex;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

pub fn render(pattern: &str, track: &Track) -> Result<String> {
    let chars: Vec<char> = pattern.chars().collect();
    let mut out = String::new();
    let mut index = 0usize;
    while index < chars.len() {
        match chars[index] {
            '\\' => {
                if index + 1 < chars.len() && "\\(),$%\\".contains(chars[index + 1]) {
                    out.push(chars[index + 1]);
                    index += 2;
                } else {
                    out.push('\\');
                    index += 1;
                }
            }
            '%' => {
                let mut end = index + 1;
                while end < chars.len() && chars[end] != '%' {
                    end += 1;
                }
                if end < chars.len() {
                    let field: String = chars[index + 1..end].iter().collect();
                    out.push_str(&track.display(field.trim()));
                    index = end + 1;
                } else {
                    out.push('%');
                    index += 1;
                }
            }
            '$' => {
                let mut name_end = index + 1;
                while name_end < chars.len()
                    && (chars[name_end].is_ascii_alphanumeric() || chars[name_end] == '_')
                {
                    name_end += 1;
                }
                if name_end < chars.len() && chars[name_end] == '(' {
                    let (inner, after) = match extract_call(&chars, name_end) {
                        Some(value) => value,
                        None => return Err(anyhow!("No closing bracket found.")),
                    };
                    let name: String = chars[index + 1..name_end].iter().collect();
                    let args = split_args(&inner);
                    let rendered = eval_function(&name, &args, track)?;
                    out.push_str(&rendered);
                    index = after;
                } else {
                    out.push('$');
                    index += 1;
                }
            }
            other => {
                out.push(other);
                index += 1;
            }
        }
    }
    Ok(out)
}

fn extract_call(chars: &[char], open: usize) -> Option<(String, usize)> {
    let mut depth = 0usize;
    let mut quote = false;
    let mut escaped = false;
    let mut out = String::new();
    let mut index = open;
    while index < chars.len() {
        let ch = chars[index];
        if escaped {
            out.push(ch);
            escaped = false;
            index += 1;
            continue;
        }
        match ch {
            '\\' => {
                escaped = true;
                out.push(ch);
            }
            '"' => {
                quote = !quote;
                out.push(ch);
            }
            '(' if !quote => {
                depth += 1;
                if depth > 1 {
                    out.push(ch);
                }
            }
            ')' if !quote => {
                depth -= 1;
                if depth == 0 {
                    return Some((out, index + 1));
                }
                out.push(ch);
            }
            _ => out.push(ch),
        }
        index += 1;
    }
    None
}

fn split_args(input: &str) -> Vec<String> {
    let mut args = Vec::new();
    let mut current = String::new();
    let mut depth = 0usize;
    let mut quote = false;
    let mut escaped = false;
    for ch in input.chars() {
        if escaped {
            current.push(ch);
            escaped = false;
            continue;
        }
        match ch {
            '\\' => {
                escaped = true;
                current.push(ch);
            }
            '"' => {
                quote = !quote;
                current.push(ch);
            }
            '(' if !quote => {
                depth += 1;
                current.push(ch);
            }
            ')' if !quote => {
                depth = depth.saturating_sub(1);
                current.push(ch);
            }
            ',' if depth == 0 && !quote => {
                args.push(current.trim().to_string());
                current.clear();
            }
            _ => current.push(ch),
        }
    }
    if !current.trim().is_empty() || !args.is_empty() {
        args.push(current.trim().to_string());
    }
    args
}

fn arg_value(arg: &str, track: &Track) -> Result<String> {
    let arg = arg.trim();
    if arg.len() >= 2 && arg.starts_with('"') && arg.ends_with('"') {
        Ok(arg[1..arg.len() - 1].to_string())
    } else {
        render(arg, track)
    }
}

fn raw_arg(arg: &str) -> String {
    arg.trim().trim_matches('"').to_string()
}

fn eval_function(name: &str, args: &[String], track: &Track) -> Result<String> {
    let lower = name.to_ascii_lowercase();
    let values: Vec<String> = args
        .iter()
        .map(|arg| arg_value(arg, track))
        .collect::<Result<_>>()?;
    let first = values.first().cloned().unwrap_or_default();
    let result = match lower.as_str() {
        "format" | "formatvalue" => render(
            &raw_arg(args.first().map(String::as_str).unwrap_or("")),
            track,
        )?,
        "upper" => first.to_uppercase(),
        "lower" => first.to_lowercase(),
        "caps" | "titlecase" | "title_case" => title_case(
            &first,
            values.get(1).map(String::as_str).unwrap_or("Mixed Case"),
            values.get(2).map(String::as_str).unwrap_or("., !"),
        ),
        "strip" => first.trim().to_string(),
        "replace" => replace(
            &first,
            values.get(1).map(String::as_str).unwrap_or(""),
            values.get(2).map(String::as_str).unwrap_or(""),
            bool_arg(values.get(3)),
            bool_arg(values.get(4)),
        ),
        "num" => number(&first, values.get(1).map(String::as_str).unwrap_or("1")),
        "len" | "len_" => first.chars().count().to_string(),
        "left" => slice_left(&first, int_arg(values.get(1)).unwrap_or(0)),
        "right" => slice_right(&first, int_arg(values.get(1)).unwrap_or(0)),
        "mid" => slice_mid(
            &first,
            int_arg(values.get(1)).unwrap_or(0),
            int_arg(values.get(2)).unwrap_or(0),
        ),
        "char" => first
            .chars()
            .next()
            .map(|ch| (ch as u32).to_string())
            .unwrap_or_default(),
        "add" => numeric(&values, |a, b| a + b)?,
        "sub" => numeric(&values, |a, b| a - b)?,
        "mul" => numeric(&values, |a, b| a * b)?,
        "div" => {
            let divisor = float_arg(values.get(1)).ok_or_else(|| anyhow!("SYNTAX ERROR"))?;
            if divisor == 0.0 {
                return Err(anyhow!("Cannot divide by zero."));
            }
            clean_float(float_arg(values.first()).unwrap_or(0.0) / divisor)
        }
        "mod" => {
            let divisor = float_arg(values.get(1)).ok_or_else(|| anyhow!("SYNTAX ERROR"))?;
            if divisor == 0.0 {
                return Err(anyhow!("Cannot divide by zero."));
            }
            clean_float(float_arg(values.first()).unwrap_or(0.0) % divisor)
        }
        "round" => float_arg(values.first())
            .map(|value| value.round() as i64)
            .unwrap_or(0)
            .to_string(),
        "ceiling" => float_arg(values.first())
            .map(|value| value.ceil() as i64)
            .unwrap_or(0)
            .to_string(),
        "floor" => float_arg(values.first())
            .map(|value| value.floor() as i64)
            .unwrap_or(0)
            .to_string(),
        "to_num" => first
            .split(|ch: char| !(ch.is_ascii_digit() || ch == '-' || ch == '+' || ch == '.'))
            .find(|part| !part.is_empty() && part.chars().any(|ch| ch.is_ascii_digit()))
            .unwrap_or("")
            .to_string(),
        "isdigit" => {
            if first.trim().parse::<f64>().is_ok() {
                "1".to_string()
            } else {
                "0".to_string()
            }
        }
        "and" | "and_" => {
            if truth(values.first()) && truth(values.get(1)) {
                "1".to_string()
            } else {
                "0".to_string()
            }
        }
        "or" | "or_" => {
            if truth(values.first()) || truth(values.get(1)) {
                "1".to_string()
            } else {
                "0".to_string()
            }
        }
        "not" | "not_" => {
            if truth(values.first()) {
                "0".to_string()
            } else {
                "1".to_string()
            }
        }
        "equals" | "eql" => {
            if first == values.get(1).cloned().unwrap_or_default() {
                "1".to_string()
            } else {
                "0".to_string()
            }
        }
        "neql" => {
            if first != values.get(1).cloned().unwrap_or_default() {
                "1".to_string()
            } else {
                "0".to_string()
            }
        }
        "if" => {
            if truth(values.first()) {
                values.get(1).cloned().unwrap_or_default()
            } else {
                values.get(2).cloned().unwrap_or_default()
            }
        }
        "meta" | "meta_sep" => {
            let field = values.get(1).cloned().unwrap_or_default();
            let joined = track.display(&field);
            if lower == "meta_sep" {
                let separator = values.get(2).cloned().unwrap_or_else(|| ", ".to_string());
                let parts: Vec<&str> = joined.split("\\\\").collect();
                parts.join(&separator)
            } else {
                joined
            }
        }
        "crlf" => "\r\n".to_string(),
        "lf" => "\n".to_string(),
        "cr" => "\r".to_string(),
        "tab" => "\t".to_string(),
        "rand" => {
            let nanos = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map(|d| d.subsec_nanos())
                .unwrap_or(0);
            (nanos as f64 / 1_000_000_000.0).to_string()
        }
        other => return Err(anyhow!("function does not exist: {other}")),
    };
    Ok(result)
}

pub fn apply_field_function(
    name: &str,
    args: &[String],
    value: &str,
    track: &Track,
) -> Result<String> {
    let lower = name.to_ascii_lowercase();
    match lower.as_str() {
        "remove_fields" => Ok(String::new()),
        "format" | "formatvalue" => render(
            &raw_arg(args.first().map(String::as_str).unwrap_or("")),
            track,
        ),
        "num" => Ok(number(
            value,
            args.first().map(String::as_str).unwrap_or("1"),
        )),
        "upper" => Ok(value.to_uppercase()),
        "lower" => Ok(value.to_lowercase()),
        "strip" => Ok(value.trim().to_string()),
        "titlecase" | "caps" | "title_case" => Ok(title_case(
            value,
            args.first().map(String::as_str).unwrap_or("Mixed Case"),
            args.get(1).map(String::as_str).unwrap_or("., !"),
        )),
        "replace" => Ok(replace(
            value,
            args.first().map(String::as_str).unwrap_or(""),
            args.get(1).map(String::as_str).unwrap_or(""),
            bool_arg(args.get(2)),
            bool_arg(args.get(3)),
        )),
        "len" | "len_" => Ok(value.chars().count().to_string()),
        "left" => Ok(slice_left(value, int_arg(args.first()).unwrap_or(0))),
        "right" => Ok(slice_right(value, int_arg(args.first()).unwrap_or(0))),
        "mid" => Ok(slice_mid(
            value,
            int_arg(args.first()).unwrap_or(0),
            int_arg(args.get(1)).unwrap_or(0),
        )),
        "to_num" => Ok(value
            .split(|ch: char| !(ch.is_ascii_digit() || ch == '-' || ch == '+' || ch == '.'))
            .find(|part| !part.is_empty() && part.chars().any(|ch| ch.is_ascii_digit()))
            .unwrap_or("")
            .to_string()),
        other => {
            let mut synthetic = vec![value.to_string()];
            synthetic.extend(args.iter().cloned());
            eval_function(other, &synthetic, track)
        }
    }
}

fn title_case(value: &str, kind: &str, delimiters: &str) -> String {
    match kind.to_ascii_lowercase().as_str() {
        "upper case" | "upper" => value.to_uppercase(),
        "lower case" | "lower" => value.to_lowercase(),
        _ => {
            let mut out = String::with_capacity(value.len());
            let mut capitalise = true;
            for ch in value.chars() {
                if capitalise && ch.is_alphabetic() {
                    out.extend(ch.to_uppercase());
                    capitalise = false;
                } else {
                    out.extend(ch.to_lowercase());
                }
                if delimiters.contains(ch) || ch.is_whitespace() {
                    capitalise = true;
                }
            }
            out
        }
    }
}

fn number(value: &str, width: &str) -> String {
    let (number, total) = value
        .split_once('/')
        .map(|(a, b)| (a.trim(), Some(b.trim())))
        .unwrap_or((value.trim(), None));
    let parsed = number.parse::<i64>().unwrap_or(0);
    let width = width.trim().parse::<usize>().unwrap_or(1).max(1);
    let formatted = format!("{parsed:0width$}");
    match total {
        Some(total) if !total.is_empty() => format!("{formatted}/{total}"),
        _ => formatted,
    }
}

fn replace(
    value: &str,
    search: &str,
    replacement: &str,
    match_case: bool,
    whole_word: bool,
) -> String {
    if search.is_empty() {
        return value.to_string();
    }
    let mut out = String::with_capacity(value.len());
    let mut index = 0usize;
    let haystack = if match_case {
        value.to_string()
    } else {
        value.to_lowercase()
    };
    let needle = if match_case {
        search.to_string()
    } else {
        search.to_lowercase()
    };
    while index <= value.len() {
        let Some(found) = haystack[index..].find(&needle) else {
            out.push_str(&value[index..]);
            break;
        };
        let start = index + found;
        let end = start + needle.len();
        let word_ok = !whole_word
            || (start == 0
                || !value[..start]
                    .chars()
                    .last()
                    .is_some_and(|c| c.is_alphanumeric()))
                && (end == value.len()
                    || !value[end..]
                        .chars()
                        .next()
                        .is_some_and(|c| c.is_alphanumeric()));
        if word_ok {
            out.push_str(&value[index..start]);
            out.push_str(replacement);
            index = end;
        } else {
            let next = value[start..]
                .chars()
                .next()
                .map(char::len_utf8)
                .unwrap_or(1);
            out.push_str(&value[index..start + next]);
            index = start + next;
        }
        if index > value.len() {
            break;
        }
    }
    out
}

fn numeric(values: &[String], op: impl Fn(f64, f64) -> f64) -> Result<String> {
    let left = float_arg(values.first()).ok_or_else(|| anyhow!("SYNTAX ERROR"))?;
    let right = float_arg(values.get(1)).ok_or_else(|| anyhow!("SYNTAX ERROR"))?;
    Ok(clean_float(op(left, right)))
}

fn clean_float(value: f64) -> String {
    if !value.is_finite() {
        return String::new();
    }
    if value.fract() == 0.0 {
        format!("{}", value as i64)
    } else {
        let text = format!("{value}");
        text.trim_end_matches('0').trim_end_matches('.').to_string()
    }
}

fn float_arg(value: Option<&String>) -> Option<f64> {
    value.and_then(|value| value.trim().parse::<f64>().ok())
}

fn int_arg(value: Option<&String>) -> Option<usize> {
    value.and_then(|value| value.trim().parse::<usize>().ok())
}

fn bool_arg(value: Option<&String>) -> bool {
    value
        .map(|value| {
            matches!(
                value.trim().to_ascii_lowercase().as_str(),
                "1" | "true" | "yes"
            )
        })
        .unwrap_or(false)
}

fn truth(value: Option<&String>) -> bool {
    value
        .map(|value| !value.is_empty() && value != "0")
        .unwrap_or(false)
}

fn slice_left(value: &str, len: usize) -> String {
    value.chars().take(len).collect()
}

fn slice_right(value: &str, len: usize) -> String {
    let count = value.chars().count();
    value.chars().skip(count.saturating_sub(len)).collect()
}

fn slice_mid(value: &str, start: usize, len: usize) -> String {
    value.chars().skip(start).take(len).collect()
}

pub fn safe_name(value: &str) -> String {
    let cleaned: String = value
        .chars()
        .filter(|ch| !matches!(ch, '/' | '\\' | '*' | '?' | '"' | '|' | ':'))
        .collect();
    if cleaned.is_empty() {
        "\"\"".to_string()
    } else {
        cleaned
    }
}

pub fn filename_to_tag(pattern: &str, filename: &str) -> BTreeMap<String, Vec<String>> {
    let stem = match filename.rsplit_once('.') {
        Some((stem, _ext)) => stem,
        None => filename,
    };
    let name_parts: Vec<&str> = stem.split('/').filter(|part| !part.is_empty()).collect();
    let pattern_parts: Vec<&str> = pattern.split('/').filter(|part| !part.is_empty()).collect();
    if pattern_parts.is_empty() || name_parts.is_empty() {
        return BTreeMap::new();
    }
    let start = name_parts.len().saturating_sub(pattern_parts.len());
    let mut fields: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for (pattern_part, name_part) in pattern_parts.iter().zip(name_parts[start..].iter()) {
        let (regex, names) = pattern_regex(pattern_part);
        let Ok(regex) = Regex::new(&format!("^{regex}$")) else {
            continue;
        };
        let Some(captures) = regex.captures(name_part) else {
            continue;
        };
        for (index, name) in names.iter().enumerate() {
            if let Some(value) = captures.get(index + 1) {
                fields
                    .entry(name.clone())
                    .or_default()
                    .push(value.as_str().to_string());
            }
        }
    }
    fields.retain(|name, _| name != "dummy");
    fields
}

fn pattern_regex(pattern: &str) -> (String, Vec<String>) {
    let mut regex = String::new();
    let mut names = Vec::new();
    let chars: Vec<char> = pattern.chars().collect();
    let mut index = 0usize;
    while index < chars.len() {
        if chars[index] == '%' {
            if let Some(end) = chars[index + 1..].iter().position(|ch| *ch == '%') {
                let name: String = chars[index + 1..index + 1 + end].iter().collect();
                names.push(name);
                let last = index + 1 + end + 1 == chars.len();
                regex.push_str(if last { "(.*)" } else { "(.*?)" });
                index = index + end + 2;
                continue;
            }
        }
        let mut literal = String::new();
        literal.push(chars[index]);
        regex.push_str(&regex::escape(&literal));
        index += 1;
    }
    (regex, names)
}

pub fn tag_to_filename(
    pattern: &str,
    track: &Track,
    include_extension: bool,
    is_dir: bool,
) -> Result<PathBuf> {
    let rendered = render(pattern, track)?;
    let absolute = rendered.starts_with('/');
    let components: Vec<String> = rendered
        .split('/')
        .filter(|part| !part.is_empty())
        .map(safe_name)
        .collect();
    let slash_count = pattern.matches('/').count();
    let mut base = track
        .path
        .parent()
        .map(Path::to_path_buf)
        .unwrap_or_default();
    if !absolute && slash_count > 0 {
        let levels = slash_count + usize::from(is_dir);
        for _ in 0..levels {
            if let Some(parent) = base.parent() {
                base = parent.to_path_buf();
            }
        }
    }
    let mut result = if absolute { PathBuf::from("/") } else { base };
    for component in &components {
        result.push(component);
    }
    if include_extension && !is_dir {
        if result.extension().is_none() {
            if let Some(ext) = track.path.extension() {
                result.set_extension(ext);
            }
        }
    }
    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::metadata::Track;
    use std::collections::BTreeMap;

    fn track() -> Track {
        let mut fields = BTreeMap::new();
        fields.insert("artist".to_string(), vec!["Artist".to_string()]);
        fields.insert("title".to_string(), vec!["Title".to_string()]);
        fields.insert("track".to_string(), vec!["7".to_string()]);
        fields.insert(
            "__filename".to_string(),
            vec!["Artist - Title.mp3".to_string()],
        );
        Track {
            path: PathBuf::from("/music/Artist - Title.mp3"),
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
            backend: crate::metadata::Backend::TagLib,
        }
    }

    #[test]
    fn renders_fields_and_functions() {
        let track = track();
        assert_eq!(
            render("%artist% - %title%", &track).unwrap(),
            "Artist - Title"
        );
        assert_eq!(render("$num(%track%,2)", &track).unwrap(), "07");
        assert_eq!(render("$upper(%artist%)", &track).unwrap(), "ARTIST");
    }

    #[test]
    fn parses_filename_patterns() {
        let tags = filename_to_tag("%artist% - %title%", "/music/Artist - Title.mp3");
        assert_eq!(tags["artist"], vec!["Artist"]);
        assert_eq!(tags["title"], vec!["Title"]);
    }

    #[test]
    fn builds_tag_filename() {
        let track = track();
        let path = tag_to_filename("%artist% - %title%", &track, true, false).unwrap();
        assert_eq!(path, PathBuf::from("/music/Artist - Title.mp3"));
    }
}
