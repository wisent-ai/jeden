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

/// The non-empty value of `key` in the env file at `path`. An absent file
/// holds no value; one that cannot be read names itself and the cause, where
/// it used to read as unconfigured and offer to set keys it already holds.
fn env_file_value(path: &Path, key: &str) -> Result<Option<String>, String> {
    let text = match fs::read_to_string(path) {
        Ok(text) => text,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(format!("{} cannot be read: {error}", path.display())),
    };
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
                return Ok(Some(value));
            }
        }
    }
    Ok(None)
}

/// Effective value for a router key: the process environment wins, then the
/// persisted `~/.jeden/.env` file. Empty values count as unconfigured; an
/// env file that cannot be read is an error, not "unconfigured".
pub(super) fn configured_value(key: &str) -> Result<Option<String>, String> {
    match std::env::var(key)
        .ok()
        .filter(|value| !value.trim().is_empty())
    {
        Some(value) => Ok(Some(value)),
        None => env_file_value(&env_file_path(), key),
    }
}

/// True when Brama's address resolves for this process: from the caller's
/// environment, Stado's service directory, or `~/.jeden/.env`. Drives the
/// welcome tip.
pub(crate) fn brama_router_configured(_cwd: &Path) -> bool {
    crate::agent::credential::brama_url().is_some()
}

/// Prefill for an INPUT row, read from the nearest `.env.example` in the
/// working directory or one of its ancestors, so subdirectories of a
/// repository work too.
pub(super) fn example_prefill(cwd: &Path, key: &str) -> Result<Option<String>, String> {
    match cwd
        .ancestors()
        .map(|dir| dir.join(".env.example"))
        .find(|candidate| candidate.is_file())
    {
        Some(candidate) => env_file_value(&candidate, key),
        None => Ok(None),
    }
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
    // Only a missing file starts empty: rewriting one that could not be read
    // would replace every line it holds with these keys alone.
    let existing = match fs::read_to_string(&path) {
        Ok(existing) => existing,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => String::new(),
        Err(error) => return Err(format!("cannot read {}: {error}", path.display())),
    };
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
