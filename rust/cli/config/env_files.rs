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

/// A value as an env file writes it: trimmed, a trailing ` #` comment cut
/// only outside quotes, quoted JSON strings decoded, and `\n` in unquoted or
/// single-quoted values read as a newline.
fn parse_env_value(raw: &str) -> Result<String, String> {
    let value = raw.trim_end();
    let prefix = value.len() - value.trim_start().len();
    let mut quote = None;
    let mut escaped = false;
    let mut preceding_space = None;
    let mut comment = None;
    for (offset, byte) in value.bytes().enumerate() {
        if escaped {
            escaped = false;
            preceding_space = None;
            continue;
        }
        if offset == prefix && matches!(byte, b'"' | b'\'') {
            quote = Some(byte);
            preceding_space = None;
            continue;
        }
        if quote == Some(b'"') && byte == b'\\' {
            escaped = true;
            preceding_space = None;
            continue;
        }
        if quote == Some(byte) {
            quote = None;
        } else if quote.is_none() && byte == b'#' && preceding_space.is_some() {
            comment = preceding_space;
            break;
        }
        preceding_space = (byte == b' ').then_some(offset);
    }
    let value = match comment {
        Some(index) => value[..index].trim(),
        None => value.trim(),
    };
    match value.as_bytes().first() {
        Some(b'"') => serde_json::from_str(value)
            .map_err(|error| format!("double-quoted value is not valid JSON: {error}")),
        Some(b'\'') => value
            .strip_prefix('\'')
            .and_then(|inner| inner.strip_suffix('\''))
            .map(|inner| inner.replace("\\n", "\n"))
            .ok_or_else(|| "single-quoted value has no closing quote".to_string()),
        _ => Ok(value.replace("\\n", "\n")),
    }
}

/// One `KEY=value` line of an env file.
pub(crate) struct Declaration<'a> {
    /// Where the line stands among the file's lines.
    pub(crate) line: usize,
    pub(crate) key: &'a str,
    pub(crate) value: String,
}

/// Every declaration of the env file `path` holding `text`, in order. Blank
/// lines and `#` comments declare nothing. Any other line that is not
/// `KEY=value` with a key is refused naming the file and the line: skipped,
/// the variable it was meant to set was silently missing from every run, and
/// the setup wizard offered to set a key the file already tried to hold.
pub(crate) fn declarations<'a>(path: &Path, text: &'a str) -> Result<Vec<Declaration<'a>>, String> {
    let mut declared = Vec::new();
    for (line, written) in text.lines().enumerate() {
        let trimmed = written.trim();
        if trimmed.is_empty() || trimmed.starts_with('#') {
            continue;
        }
        let key = match written.split_once('=') {
            Some((key, raw_value)) if !key.is_empty() && !key.contains(char::is_whitespace) => {
                declared.push(Declaration {
                    line,
                    key,
                    value: parse_env_value(raw_value).map_err(|reason| {
                        format!(
                            "{}: line index {line} (zero-based) has an unreadable value: {reason}",
                            path.display()
                        )
                    })?,
                });
                continue;
            }
            Some((key, _)) => key,
            None => written,
        };
        return Err(format!(
            "{}: line index {line} (zero-based) holds `{written}`, not KEY=value (`{key}` is not a variable name); a line is a declaration, a `#` comment or blank",
            path.display()
        ));
    }
    Ok(declared)
}

/// Set every variable `path` declares that the environment does not already
/// name. A file that is not there declares nothing; one that cannot be read,
/// or holds a line that is not a declaration, is refused with its path.
fn load_env_path(path: &Path, loaded: &mut Vec<String>) -> Result<(), String> {
    let text = match fs::read_to_string(path) {
        Ok(text) => text,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(format!("{}: {error}", path.display())),
    };
    for declaration in declarations(path, &text)? {
        if env::var_os(declaration.key).is_some() {
            continue;
        }
        env::set_var(declaration.key, declaration.value);
        loaded.push(declaration.key.to_string());
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
