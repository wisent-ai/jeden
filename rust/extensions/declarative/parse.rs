//! Reading a definition file without letting it reach outside itself.
//!
//! Split out of `extensions/declarative.rs`, which had grown past the module
//! line cap.

use super::{MAX_ASSETS_PER_SKILL, MAX_DEFINITIONS};
use regex::Regex;
use serde_json::{Map, Value};
use std::path::{Path, PathBuf};
use std::fs;

pub(super) fn valid_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= 80
        && id
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.'))
}

pub(super) fn collect(root: &Path, extensions: &[&str], out: &mut Vec<PathBuf>) {
    if out.len() >= MAX_DEFINITIONS || !root.exists() {
        return;
    }
    if root.is_file() {
        if root
            .extension()
            .and_then(|value| value.to_str())
            .is_some_and(|extension| extensions.contains(&extension))
        {
            out.push(root.to_path_buf());
        }
        return;
    }
    let Ok(entries) = fs::read_dir(root) else {
        return;
    };
    let mut paths = entries
        .flatten()
        .map(|entry| entry.path())
        .collect::<Vec<_>>();
    paths.sort();
    for path in paths {
        collect(&path, extensions, out);
        if out.len() >= MAX_DEFINITIONS {
            break;
        }
    }
}

pub(super) fn parse_frontmatter(text: &str) -> Result<(Map<String, Value>, String), String> {
    let normalized = text.replace("\r\n", "\n");
    if !normalized.starts_with("---\n") {
        return Ok((Map::new(), normalized));
    }
    let tail = &normalized[4..];
    let end = tail
        .find("\n---\n")
        .ok_or("unterminated YAML frontmatter")?;
    let metadata: Value = serde_yaml::from_str(&tail[..end]).map_err(|error| error.to_string())?;
    let metadata = metadata
        .as_object()
        .cloned()
        .ok_or("frontmatter must be an object")?;
    Ok((metadata, tail[end + 5..].to_string()))
}

pub(super) fn string_list(value: Option<&Value>) -> Result<Vec<String>, String> {
    match value {
        None | Some(Value::Null) => Ok(Vec::new()),
        Some(Value::String(value)) => Ok(vec![value.clone()]),
        Some(Value::Array(values)) => values
            .iter()
            .map(|value| {
                value
                    .as_str()
                    .map(str::to_string)
                    .ok_or_else(|| "list entries must be strings".to_string())
            })
            .collect(),
        Some(_) => Err("expected a string or string array".into()),
    }
}

pub(super) fn validate_matchers(matchers: &[String]) -> Result<(), String> {
    for matcher in matchers {
        Regex::new(matcher).map_err(|error| format!("invalid matcher {matcher:?}: {error}"))?;
    }
    Ok(())
}

pub(super) fn matches(matchers: &[String], prompt: &str) -> bool {
    matchers
        .iter()
        .any(|matcher| Regex::new(matcher).is_ok_and(|regex| regex.is_match(prompt)))
}

pub(super) fn safe_assets(skill_file: &Path, raw: Option<&Value>) -> Result<Vec<PathBuf>, String> {
    let relative = string_list(raw)?;
    if relative.len() > MAX_ASSETS_PER_SKILL {
        return Err(format!("skill exceeds {MAX_ASSETS_PER_SKILL} assets"));
    }
    let root = skill_file.parent().unwrap_or_else(|| Path::new("."));
    let canonical_root = fs::canonicalize(root).map_err(|error| error.to_string())?;
    let mut assets = Vec::new();
    for asset in relative {
        let candidate = root.join(&asset);
        let canonical = fs::canonicalize(&candidate)
            .map_err(|error| format!("skill asset {asset}: {error}"))?;
        if !canonical.starts_with(&canonical_root) || !canonical.is_file() {
            return Err(format!("unsafe skill asset: {asset}"));
        }
        assets.push(canonical);
    }
    Ok(assets)
}

pub(crate) fn skill_file_id(path: &Path) -> String {
    let file = path
        .file_stem()
        .and_then(|value| value.to_str())
        .unwrap_or("skill");
    if file.eq_ignore_ascii_case("skill") {
        path.parent()
            .and_then(Path::file_name)
            .and_then(|value| value.to_str())
            .unwrap_or(file)
            .to_string()
    } else {
        file.to_string()
    }
}
