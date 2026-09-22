use serde_json::{json, Map, Value};
use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use url::Url;

pub(crate) fn dirs_home() -> PathBuf {
    env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("."))
}

pub(crate) fn project_config_path(cwd: &Path) -> PathBuf {
    cwd.join(".jeden/config.json")
}

pub(crate) fn read_json_value(path: &Path) -> Value {
    fs::read_to_string(path)
        .ok()
        .and_then(|text| serde_json::from_str::<Value>(&text).ok())
        .unwrap_or_else(|| json!({}))
}

pub(crate) fn write_json_value(path: &Path, value: &Value) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }
    let text = serde_json::to_string_pretty(value).map_err(|e| e.to_string())? + "\n";
    fs::write(path, text).map_err(|e| e.to_string())
}

pub(crate) fn merged_config(cwd: &Path) -> Value {
    let mut merged = match read_json_value(&dirs_home().join(".jeden/config.json")) {
        Value::Object(map) => map,
        _ => Map::new(),
    };
    if let Value::Object(project) = read_json_value(&project_config_path(cwd)) {
        for (key, value) in project {
            merged.insert(key, value);
        }
    }
    Value::Object(merged)
}

pub(crate) fn is_plain_object(value: &Value) -> bool {
    matches!(value, Value::Object(_))
}

pub(crate) fn file_url(path: &Path) -> String {
    Url::from_file_path(path)
        .map(|url| url.to_string())
        .unwrap_or_else(|_| format!("file://{}", path.display()))
}

pub(crate) fn split_head(args: &str) -> (&str, &str) {
    let text = args.trim();
    if text.is_empty() {
        return ("", "");
    }
    match text.find(char::is_whitespace) {
        Some(index) => (&text[..index], text[index..].trim()),
        None => (text, ""),
    }
}

pub(crate) fn now_millis() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}

pub(crate) fn now_text() -> String {
    now_millis().to_string()
}

pub(crate) fn split_args(value: &str) -> Vec<String> {
    let mut args = Vec::new();
    let mut current = String::new();
    let mut quote: Option<char> = None;
    let mut escaped = false;
    for ch in value.trim().chars() {
        if escaped {
            current.push(ch);
            escaped = false;
            continue;
        }
        if ch == '\\' {
            escaped = true;
            continue;
        }
        if let Some(active) = quote {
            if ch == active {
                quote = None;
            } else {
                current.push(ch);
            }
            continue;
        }
        if ch == '"' || ch == '\'' {
            quote = Some(ch);
            continue;
        }
        if ch.is_whitespace() {
            if !current.is_empty() {
                args.push(std::mem::take(&mut current));
            }
            continue;
        }
        current.push(ch);
    }
    if !current.is_empty() {
        args.push(current);
    }
    args
}

/// Parse a duration suffix into milliseconds. Only `ms` and `s` are supported;
/// the factors are encapsulated by `Duration` so no bare numeric literal is
/// needed. Larger units are intentionally omitted.
pub(crate) fn parse_duration_ms(value: &str) -> Option<u64> {
    let lower = value.trim().to_ascii_lowercase();
    let split = lower.find(|ch: char| !ch.is_ascii_digit())?;
    let amount = lower[..split].parse::<u64>().ok()?;
    match &lower[split..] {
        "ms" => Some(amount),
        "s" => Some(Duration::from_secs(amount).as_millis() as u64),
        _ => None,
    }
}

/// Resolve `target` against `cwd`: absolute paths are returned as-is, relative
/// paths are joined onto `cwd`.
pub(crate) fn resolve_cwd_path(cwd: &Path, target: &str) -> PathBuf {
    let path = PathBuf::from(target);
    if path.is_absolute() {
        path
    } else {
        cwd.join(path)
    }
}
