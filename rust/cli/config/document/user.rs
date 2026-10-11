//! Reading and writing the operator's own configuration file without ever
//! replacing something unreadable with something fresh.
//!
//! Split out of `cli/config/mod.rs`, which had grown past the module line cap.

use super::super::migrations;
use super::layers::read_config_layer;
use super::migrate::migrate_config_file;
use crate::cli::config::document::migrate::CONFIG_SCHEMA_VERSION;
use crate::{legacy_user_config_path, user_config_path};
use serde_json::{json, Value};
use std::fs;
use std::path::PathBuf;

/// The version marker a legacy user file may carry beside `schemaVersion`;
/// neither is a setting, so a legacy file holding only these is retired.
const LEGACY_VERSION_KEY: &str = "version";

/// The writable user layer for operations that must never replace unreadable
/// or malformed existing state with a fresh configuration.
pub(crate) fn read_user_writable_config_strict() -> Result<Value, String> {
    let current = read_config_layer(&user_config_path())?;
    if current
        .as_ref()
        .and_then(Value::as_object)
        .is_some_and(|object| !object.is_empty())
    {
        return Ok(current.expect("checked as present"));
    }
    if let Some(legacy) = read_config_layer(&legacy_user_config_path())? {
        return Ok(legacy);
    }
    Ok(current.unwrap_or_else(|| json!({})))
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
    // The legacy layer is read before anything is written: a legacy file that
    // cannot be read or parsed refuses the whole write, so neither file
    // changes, rather than publishing the new layer and then failing to
    // retire the old one.
    let legacy = read_config_layer(&legacy_user_config_path())?;
    if path.exists() {
        migrate_config_file(&path)?;
    }
    let mut versioned = value.clone();
    let object = versioned
        .as_object_mut()
        .ok_or_else(|| "config root must be an object".to_string())?;
    object.insert(
        migrations::SCHEMA_VERSION_KEY.into(),
        json!(CONFIG_SCHEMA_VERSION),
    );
    let written = object.clone();
    migrations::write_json_atomic(&path, &versioned)?;
    retire_legacy_keys(&written, legacy)?;
    Ok(path)
}

/// Drop from the legacy user file, already read as `legacy`, every key the
/// current one now carries.
fn retire_legacy_keys(
    written: &serde_json::Map<String, Value>,
    legacy: Option<Value>,
) -> Result<(), String> {
    let Some(Value::Object(mut remaining)) = legacy else {
        return Ok(());
    };
    let legacy = legacy_user_config_path();
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
        .all(|key| key == migrations::SCHEMA_VERSION_KEY || key == LEGACY_VERSION_KEY)
    {
        return fs::remove_file(&legacy).map_err(|error| {
            format!("cannot remove the superseded {}: {error}", legacy.display())
        });
    }
    migrations::write_json_atomic(&legacy, &Value::Object(remaining))
}
