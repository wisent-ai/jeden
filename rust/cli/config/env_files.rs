//! The `.env` files Jeden reads at start: the working directory's own, then
//! `~/.jeden/.env`. A variable the environment already names is never
//! replaced, so the process environment stays authoritative.

use std::env;
use std::fs;
use std::path::Path;

use crate::slash::common::dirs_home;

/// The working directory's env files, in the order they are read; an earlier
/// file wins over a later one, because a set variable is never replaced.
const WORKSPACE_ENV_FILES: &[&str] = &[".env", ".env.local", ".env.production", ".env.vercel"];

fn parse_env_value(raw: &str) -> String {
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

/// Set every variable `path` declares that the environment does not already
/// name. A file that is not there declares nothing; one that cannot be read
/// is refused with its path.
fn load_env_path(path: &Path, loaded: &mut Vec<String>) -> Result<(), String> {
    let text = match fs::read_to_string(path) {
        Ok(text) => text,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(format!("{}: {error}", path.display())),
    };
    for line in text.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() || trimmed.starts_with('#') {
            continue;
        }
        let Some((key, raw_value)) = trimmed.split_once('=') else {
            continue;
        };
        let key = key.trim();
        if key.is_empty() || env::var_os(key).is_some() {
            continue;
        }
        env::set_var(key, parse_env_value(raw_value));
        loaded.push(key.to_string());
    }
    Ok(())
}

/// Load the env files of `cwd` and of `~/.jeden`; the names of the variables
/// they supplied, sorted.
pub(crate) fn load_env_files(cwd: &Path) -> Result<Vec<String>, String> {
    let mut loaded = Vec::new();
    for name in WORKSPACE_ENV_FILES {
        load_env_path(&cwd.join(name), &mut loaded)?;
    }
    load_env_path(&dirs_home().join(".jeden/.env"), &mut loaded)?;
    loaded.sort();
    loaded.dedup();
    Ok(loaded)
}
