//! Reading and writing the operator's environment file the same way the
//! loader reads it, so what the wizard saves is what the next run sees.
//!
//! Split out of `slash/setup.rs`, which had grown past the module line cap.

use super::super::common::dirs_home;
use super::{AGENT_ID_KEY, BRAMA_URL_KEY};
use std::fs;
use std::path::{Path, PathBuf};

pub(super) fn env_file_path() -> PathBuf {
    dirs_home().join(".jeden/.env")
}

/// Parse one `KEY=value` line the same way the main env loader does: trim,
/// strip a trailing ` #` comment, unquote, and expand `\n`.
fn parse_env_line_value(raw: &str) -> String {
    let mut value = raw.trim().to_string();
    if let Some(index) = value.find(" #") {
        value.truncate(index);
        value = value.trim().to_string();
    }
    let unquoted = value
        .strip_prefix('"')
        .and_then(|v| v.strip_suffix('"'))
        .or_else(|| value.strip_prefix('\'').and_then(|v| v.strip_suffix('\'')));
    if let Some(inner) = unquoted {
        value = inner.to_string();
    }
    value.replace("\\n", "\n")
}

fn env_file_value(path: &Path, key: &str) -> Option<String> {
    let text = fs::read_to_string(path).ok()?;
    for line in text.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() || trimmed.starts_with('#') {
            continue;
        }
        let Some((name, raw_value)) = trimmed.split_once('=') else {
            continue;
        };
        if name.trim() == key {
            let value = parse_env_line_value(raw_value);
            if !value.trim().is_empty() {
                return Some(value);
            }
        }
    }
    None
}

/// Effective value for a router key: the process environment wins, then the
/// persisted `~/.jeden/.env` file. Empty values count as unconfigured.
pub(super) fn configured_value(key: &str) -> Option<String> {
    std::env::var(key)
        .ok()
        .filter(|value| !value.trim().is_empty())
        .or_else(|| env_file_value(&env_file_path(), key))
}

/// True when the required Brama model-router endpoint is available
/// from the process environment or `~/.jeden/.env`. Drives the welcome tip.
pub(crate) fn brama_router_configured(_cwd: &Path) -> bool {
    configured_value(BRAMA_URL_KEY).is_some()
}

/// Prefill for an INPUT row, read best-effort from the repository's
/// `.env.example` (cwd and up to two parents, so subdirectories work too).
pub(super) fn example_prefill(cwd: &Path, key: &str) -> Option<String> {
    let mut dir = Some(cwd);
    for _ in 0..3 {
        let candidate = dir?.join(".env.example");
        if let Some(value) = env_file_value(&candidate, key) {
            return Some(value);
        }
        dir = dir?.parent();
    }
    None
}

/// Append or update `updates` in `~/.jeden/.env`, preserving every unrelated
/// line byte-for-byte, and force the file to mode 0600. Values that would not
/// survive the loader's comment/quote parsing are written double-quoted.
fn write_env_keys(updates: &[(String, String)]) -> Result<PathBuf, String> {
    let path = env_file_path();
    let encode = |key: &str, value: &str| {
        if value.contains('#') || value.contains(char::is_whitespace) {
            format!("{key}=\"{value}\"")
        } else {
            format!("{key}={value}")
        }
    };
    let existing = fs::read_to_string(&path).unwrap_or_default();
    let mut pending: Vec<&(String, String)> = updates.iter().collect();
    let mut lines: Vec<String> = existing.lines().map(str::to_string).collect();
    for line in lines.iter_mut() {
        let trimmed = line.trim();
        if trimmed.is_empty() || trimmed.starts_with('#') {
            continue;
        }
        let Some((name, _)) = trimmed.split_once('=') else {
            continue;
        };
        let name = name.trim();
        if let Some(position) = pending.iter().position(|(key, _)| key == name) {
            let (key, value) = pending.remove(position);
            *line = encode(key, value);
        }
    }
    for (key, value) in pending {
        lines.push(encode(key, value));
    }
    let mut text = lines.join("\n");
    text.push('\n');
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|error| error.to_string())?;
    }
    fs::write(&path, text).map_err(|error| error.to_string())?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&path, fs::Permissions::from_mode(0o600))
            .map_err(|error| error.to_string())?;
    }
    // Make the values effective for this running session (catalog fetch,
    // agent identity) without requiring a restart.
    for (key, value) in updates {
        std::env::set_var(key, value);
    }
    Ok(path)
}

pub(super) fn save_brama_url(value: &str) -> Result<String, String> {
    let value = value.trim();
    if !(value.starts_with("https://") || value.starts_with("http://")) {
        return Err("BRAMA_URL must start with https:// or http://".into());
    }
    let path = write_env_keys(&[(BRAMA_URL_KEY.into(), value.into())])?;
    Ok(format!("Saved BRAMA_URL to {} (0600).", path.display()))
}

pub(super) fn save_agent_id(value: &str) -> Result<String, String> {
    let value = value.trim();
    if value.is_empty() || value.contains(char::is_whitespace) {
        return Err("WISENT_APP_AGENT_ID must be a single non-empty token".into());
    }
    let path = write_env_keys(&[(AGENT_ID_KEY.into(), value.into())])?;
    Ok(format!(
        "Saved WISENT_APP_AGENT_ID ({value}) to {} (0600).",
        path.display()
    ))
}
