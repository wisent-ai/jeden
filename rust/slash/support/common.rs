use serde_json::{json, Map, Value};
use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::Duration;
use url::Url;

use crate::tui::{PickerItem, PickerSpec};

/// The user's home directory: `HOME` when it is set and not empty, else the
/// home the user database records for this user (`std::env::home_dir`, which
/// asks it when `HOME` is unset, as under launchd or a stripped environment).
/// Never the working directory: read as `.`, every user-scope file — the
/// `.env`, `config.json`, `mcp.json`, the plugin registry, the Brama catalogue
/// cache — was read and written inside whatever project Jeden started in.
/// When neither source names a home, Jeden stops with that cause.
pub(crate) fn dirs_home() -> PathBuf {
    if let Some(home) = env::var_os("HOME").filter(|home| !home.is_empty()) {
        return PathBuf::from(home);
    }
    #[allow(deprecated)]
    let recorded = env::home_dir().filter(|home| !home.as_os_str().is_empty());
    recorded.expect(
        "HOME is unset or empty and the user database records no home directory for this user, \
         so Jeden's user files (~/.jeden) cannot be found",
    )
}

pub(crate) fn project_config_path(cwd: &Path) -> PathBuf {
    cwd.join(".jeden/config.json")
}

/// A JSON state or configuration file a slash command reads (`mcp.json`, the
/// plugin registry, collaboration and mode state, usage): a file that does
/// not exist is an empty object; one that cannot be read or does not parse is
/// refused naming the file and the cause. Read as empty, a typo in `mcp.json`
/// made every server it declares vanish, its `disabledServers` too.
pub(crate) fn read_json_value(path: &Path) -> Result<Value, String> {
    let text = match fs::read_to_string(path) {
        Ok(text) => text,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(json!({})),
        Err(error) => return Err(format!("{} cannot be read: {error}", path.display())),
    };
    serde_json::from_str::<Value>(&text)
        .map_err(|error| format!("{} is not valid JSON: {error}", path.display()))
}

/// A picker that can only say why it has nothing to offer: one disabled row
/// naming the refused operation, its cause in the detail.
pub(crate) fn refusal_picker(title: &str, operation: &str, cause: String) -> PickerSpec {
    PickerSpec::new(
        title,
        vec![PickerItem::action(operation, "")
            .detail(cause)
            .badge("error")
            .disabled(true)],
    )
}

/// Writes `value` to `path`. A file already there that cannot be read or is
/// not JSON is refused with its path and cause, never replaced: every caller
/// edits what `read_json_value` gave it; a file that changed under it since
/// is still never replaced by one entry.
pub(crate) fn write_json_value(path: &Path, value: &Value) -> Result<(), String> {
    match fs::read_to_string(path) {
        Ok(text) => {
            if let Err(error) = serde_json::from_str::<Value>(&text) {
                return Err(format!(
                    "{} is not JSON, so it is not replaced: {error}",
                    path.display()
                ));
            }
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => {
            return Err(format!(
                "cannot read {}, so it is not replaced: {error}",
                path.display()
            ))
        }
    }
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }
    let text = serde_json::to_string_pretty(value).map_err(|e| e.to_string())? + "\n";
    fs::write(path, text).map_err(|e| e.to_string())
}

/// The project configuration a slash command edits: absent is an empty
/// object; one that cannot be read, does not parse, or has no object at its
/// root is refused naming the file, so an edit never replaces every setting
/// it holds with the one being changed. What a command reads as effective
/// configuration comes from `crate::cli::config::merged_config_value`, the
/// same layers every run reads.
pub(crate) fn project_config_for_edit(cwd: &Path) -> Result<Value, String> {
    match crate::cli::config::read_config_layer(&project_config_path(cwd))? {
        Some(layer) => Ok(layer),
        None => Ok(Value::Object(Map::new())),
    }
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

/// Milliseconds since the Unix epoch as text, or why the clock has none.
pub(crate) fn now_text() -> Result<String, String> {
    crate::clock::unix_millis()
        .map(|millis| millis.to_string())
        .map_err(|error| error.to_string())
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
