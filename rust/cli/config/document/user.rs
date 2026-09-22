//! Reading and writing the operator's own configuration file without ever
//! replacing something unreadable with something fresh.
//!
//! Split out of `cli/config/mod.rs`, which had grown past the module line cap.

use super::super::migrations;
use super::layers::read_config_value;
use super::migrate::migrate_config_file;
use crate::{legacy_user_config_path, user_config_path};
use serde_json::{json, Value};
use std::fs;
use std::path::{Path, PathBuf};

fn read_user_config_file_strict(path: &Path) -> Result<Option<Value>, String> {
    let text = match fs::read_to_string(path) {
        Ok(text) => text,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => {
            return Err(format!(
                "cannot read existing user configuration {}: {error}",
                path.display()
            ))
        }
    };
    let value = match path
        .extension()
        .and_then(|extension| extension.to_str())
        .unwrap_or_default()
        .to_ascii_lowercase()
        .as_str()
    {
        "yml" | "yaml" => serde_yaml::from_str::<Value>(&text).map_err(|error| error.to_string()),
        _ => serde_json::from_str::<Value>(&text).map_err(|error| error.to_string()),
    }
    .map_err(|error| {
        format!(
            "invalid existing user configuration {}: {error}",
            path.display()
        )
    })?;
    if !value.is_object() {
        return Err(format!(
            "invalid existing user configuration {}: root must be an object",
            path.display()
        ));
    }
    Ok(Some(value))
}

/// The writable user layer for operations that must never replace unreadable
/// or malformed existing state with a fresh configuration.
pub(crate) fn read_user_writable_config_strict() -> Result<Value, String> {
    let current = read_user_config_file_strict(&user_config_path())?;
    if current
        .as_ref()
        .and_then(Value::as_object)
        .is_some_and(|object| !object.is_empty())
    {
        return Ok(current.expect("checked as present"));
    }
    if let Some(legacy) = read_user_config_file_strict(&legacy_user_config_path())? {
        return Ok(legacy);
    }
    Ok(current.unwrap_or_else(|| json!({})))
}

pub(crate) fn read_user_writable_config() -> Value {
    let current = read_config_value(&user_config_path());
    if current
        .as_object()
        .map(|map| !map.is_empty())
        .unwrap_or(false)
    {
        current
    } else {
        read_config_value(&legacy_user_config_path())
    }
}

/// Write the user's own configuration, and stop the superseded file from
/// disagreeing with it.
///
/// `~/.jeden/config.json` is the layer this file replaced. Leaving a key in
/// both is a second source of truth: `jeden config set model …` wrote the
/// new route here while the old file kept answering with a model route
/// Brama no longer serves. Every key written here is therefore removed from
/// the legacy file, and a legacy file left with nothing but its schema
/// version is deleted.
pub(crate) fn write_user_config(value: &Value) -> Result<PathBuf, String> {
    let path = user_config_path();
    if path.exists() {
        migrate_config_file(&path)?;
    }
    let mut versioned = value.clone();
    let object = versioned
        .as_object_mut()
        .ok_or_else(|| "config root must be an object".to_string())?;
    object.insert("schemaVersion".into(), json!(CONFIG_SCHEMA_VERSION));
    let written = object.clone();
    migrations::write_json_atomic(&path, &versioned)?;
    retire_legacy_keys(&written)?;
    Ok(path)
}

/// Drop from the legacy user file every key the current one now carries.
fn retire_legacy_keys(written: &serde_json::Map<String, Value>) -> Result<(), String> {
    let legacy = legacy_user_config_path();
    let Value::Object(mut remaining) = read_config_value(&legacy) else {
        return Ok(());
    };
    if remaining.is_empty() {
        return Ok(());
    }
    let before = remaining.len();
    remaining.retain(|key, _| !written.contains_key(key));
    if remaining.len() == before {
        return Ok(());
    }
    if remaining
        .keys()
        .all(|key| key == "schemaVersion" || key == "version")
    {
        return fs::remove_file(&legacy).map_err(|error| {
            format!("cannot remove the superseded {}: {error}", legacy.display())
        });
    }
    migrations::write_json_atomic(&legacy, &Value::Object(remaining))
}
