//! Reading and writing the operator's environment file the same way the
//! loader reads it, so what the wizard saves is what the next run sees.
//!
//! Split out of `slash/setup.rs`, which had grown past the module line cap.

use super::super::common::dirs_home;
use super::{AGENT_ID_KEY, BRAMA_URL_KEY};
use crate::cli::config::env_files::declarations;
use std::fs;
use std::path::{Path, PathBuf};

pub(super) fn env_file_path() -> PathBuf {
    dirs_home().join(".jeden/.env")
}

/// The first non-empty value of `key` in the env file at `path`, read by the
/// loader's own reader. An absent file holds no value; one that cannot be
/// read, or holds a line that is not a declaration, names itself and the
/// cause, where it used to read as unconfigured and offer to set keys it
/// already holds.
fn env_file_value(path: &Path, key: &str) -> Result<Option<String>, String> {
    let text = match fs::read_to_string(path) {
        Ok(text) => text,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(format!("{} cannot be read: {error}", path.display())),
    };
    Ok(declarations(path, &text)?
        .into_iter()
        .find(|declaration| declaration.key == key && !declaration.value.trim().is_empty())
        .map(|declaration| declaration.value))
}

/// Effective value for a router key: the process environment wins, then the
/// persisted `~/.jeden/.env` file. Empty values count as unconfigured; an
/// env file that cannot be read is an error, not "unconfigured".
pub(super) fn configured_value(key: &str) -> Result<Option<String>, String> {
    match std::env::var(key) {
        Ok(value) if !value.trim().is_empty() => Ok(Some(value)),
        Ok(_) | Err(std::env::VarError::NotPresent) => env_file_value(&env_file_path(), key),
        Err(error) => Err(format!(
            "{key} cannot be read from the process environment: {error}"
        )),
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
/// repository work too. A candidate that cannot be examined is refused
/// naming it: skipped, a farther `.env.example` prefilled the row instead.
pub(super) fn example_prefill(cwd: &Path, key: &str) -> Result<Option<String>, String> {
    for candidate in cwd.ancestors().map(|dir| dir.join(".env.example")) {
        match std::fs::metadata(&candidate) {
            Ok(metadata) if metadata.is_file() => return env_file_value(&candidate, key),
            Ok(_not_file) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => {
                return Err(format!(
                    "{} cannot be examined: {error}",
                    candidate.display()
                ))
            }
        }
    }
    Ok(None)
}

/// Append or update `updates` in `~/.jeden/.env`, preserving every unrelated
/// line byte-for-byte, and force the file to mode 0600. Values that would not
/// survive the loader's comment/quote parsing are written double-quoted.
fn write_env_keys(updates: &[(String, String)]) -> Result<PathBuf, String> {
    let path = env_file_path();
    let encode = |key: &str, value: &str| -> Result<String, String> {
        if value.contains('#')
            || value.contains(char::is_whitespace)
            || value.contains('"')
            || value.contains('\\')
            || value.starts_with('\'')
        {
            let quoted = serde_json::to_string(value)
                .map_err(|error| format!("cannot encode {key} for {}: {error}", path.display()))?;
            Ok(format!("{key}={quoted}"))
        } else {
            Ok(format!("{key}={value}"))
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
    let mut parsed = declarations(&path, &existing)?.into_iter().peekable();
    let mut text = String::with_capacity(existing.len());
    for (line, written) in existing.split_inclusive('\n').enumerate() {
        if let Some(declaration) = parsed.next_if(|entry| entry.line == line) {
            if let Some(position) = pending.iter().position(|(key, _)| key == declaration.key) {
                let (key, value) = pending.remove(position);
                text.push_str(&encode(key, value)?);
                if written.ends_with("\r\n") {
                    text.push_str("\r\n");
                } else if written.ends_with('\n') {
                    text.push('\n');
                }
                continue;
            }
        }
        text.push_str(written);
    }
    for (key, value) in pending {
        if !text.is_empty() && !text.ends_with('\n') {
            text.push('\n');
        }
        text.push_str(&encode(key, value)?);
        text.push('\n');
    }
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .map_err(|error| format!("cannot create {}: {error}", parent.display()))?;
    }
    fs::write(&path, text).map_err(|error| format!("cannot write {}: {error}", path.display()))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(
            &path,
            fs::Permissions::from_mode(
                crate::tool_runtime::runtime_ops::platform::mode::OWNER_READ_WRITE,
            ),
        )
        .map_err(|error| format!("cannot make {} private: {error}", path.display()))?;
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
