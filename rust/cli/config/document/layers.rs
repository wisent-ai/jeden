//! Finding the configuration files that apply here, and merging them in the
//! right order.
//!
//! Split out of `cli/config/mod.rs`, which had grown past the module line cap.

use crate::{config_path, legacy_user_config_path, user_config_path};
use serde::Deserialize;
use serde_json::{json, Value};
use std::fs;
use std::io::ErrorKind;
use std::path::{Path, PathBuf};

/// The extension of a layer written as JSON; every other layer is YAML.
const JSON_EXTENSION: &str = "json";

/// One configuration layer as the file holds it. A file that does not exist
/// is no layer (`None`). One that cannot be read, does not parse, or whose
/// root is not an object is refused naming the file and the cause: read as
/// `{}`, a typo or a permission problem dropped every setting in it (the
/// model route, task limits, the Tama registry, disabled extensions) without
/// a word.
pub(crate) fn read_config_layer(path: &Path) -> Result<Option<Value>, String> {
    let text = match fs::read_to_string(path) {
        Ok(text) => text,
        Err(error) if error.kind() == ErrorKind::NotFound => return Ok(None),
        Err(error) => {
            return Err(format!(
                "configuration {} cannot be read: {error}",
                path.display()
            ))
        }
    };
    let written_as_json = path
        .extension()
        .is_some_and(|extension| extension == JSON_EXTENSION);
    let parsed = if written_as_json {
        serde_json::from_str::<Value>(&text).map_err(|error| error.to_string())
    } else {
        serde_yaml::from_str::<Value>(&text).map_err(|error| error.to_string())
    };
    let value = parsed
        .map_err(|error| format!("configuration {} is not valid: {error}", path.display()))?;
    if !value.is_object() {
        return Err(format!(
            "configuration {} is not valid: its root must be an object",
            path.display()
        ));
    }
    Ok(Some(value))
}

/// One layer for merging: an absent file is an empty layer. `jeden` refuses
/// to start while any layer is unreadable or invalid (`check_config`), so a
/// failure here means the file broke during the run; the run stops with the
/// cause instead of continuing without the file's settings.
pub(crate) fn read_config_value(path: &Path) -> Value {
    match read_config_layer(path) {
        Ok(Some(value)) => value,
        Ok(None) => json!({}),
        Err(error) => panic!("{error}"),
    }
}

/// One layer as `T`; a layer whose values do not fit `T` stops the run naming
/// the file, as `read_config_value` does.
pub(crate) fn read_config_typed<T: for<'a> Deserialize<'a>>(path: &Path) -> T {
    match serde_json::from_value(read_config_value(path)) {
        Ok(typed) => typed,
        Err(error) => panic!(
            "configuration {} does not hold valid settings: {error}",
            path.display()
        ),
    }
}

pub(super) fn global_config_layer_paths() -> Vec<PathBuf> {
    vec![legacy_user_config_path(), user_config_path()]
}

fn project_config_layer_paths(cwd: &Path) -> Vec<PathBuf> {
    vec![config_path(cwd)]
}

/// User layers first, then the project's own file.
///
/// In the home directory the project layer resolves to
/// `~/.jeden/config.json`, which is also the legacy user layer, and applying
/// it twice put the older file last: `jeden config set model …` wrote
/// `~/.jeden/config.yml`, `jeden config get model` still answered with the
/// legacy value, and a run started in the home directory used a model route
/// Brama no longer serves. One file is one layer, in its user position, so
/// the current file keeps overriding it.
pub(crate) fn config_layer_paths(cwd: &Path) -> Vec<PathBuf> {
    let mut paths = global_config_layer_paths();
    for path in project_config_layer_paths(cwd) {
        if !paths.contains(&path) {
            paths.push(path);
        }
    }
    paths
}

fn deep_merge_value(base: &mut Value, overlay: Value) {
    match (base, overlay) {
        (Value::Object(base), Value::Object(overlay)) => {
            for (key, value) in overlay {
                if let Some(existing) = base.get_mut(&key) {
                    deep_merge_value(existing, value);
                } else {
                    base.insert(key, value);
                }
            }
        }
        (base, overlay) => *base = overlay,
    }
}

pub(crate) fn merged_config_value(cwd: &Path) -> Value {
    let mut merged = json!({});
    for path in config_layer_paths(cwd) {
        deep_merge_value(&mut merged, read_config_value(&path));
    }
    merged
}

/// The user's own layers merged, without the project's: what a screen shows
/// as inherited from user configuration.
pub(crate) fn user_config_value() -> Value {
    let mut merged = json!({});
    for path in global_config_layer_paths() {
        deep_merge_value(&mut merged, read_config_value(&path));
    }
    merged
}

pub(crate) fn config_value_at<'a>(value: &'a Value, key: &str) -> Option<&'a Value> {
    let mut current = value;
    for part in key.split('.') {
        if part.is_empty() {
            return None;
        }
        current = current.get(part)?;
    }
    Some(current)
}

/// The texts the config list `key` holds in `value`; none when it states
/// none. A value in another shape, or an entry that is not text, is refused
/// naming `key`: read as empty, a disabledExtensions in the wrong shape
/// loaded every extension it was meant to keep out, and a debugger adapter
/// started without the arguments it was configured with.
pub(crate) fn config_texts<'a>(value: &'a Value, key: &str) -> Result<Vec<&'a str>, String> {
    match value {
        Value::Null => Ok(Vec::new()),
        Value::Array(entries) => entries
            .iter()
            .map(|entry| {
                entry
                    .as_str()
                    .ok_or_else(|| format!("config {key} holds an entry that is not text: {entry}"))
            })
            .collect(),
        other => Err(format!("config {key} must be a list of text; it holds {other}")),
    }
}

/// Set `key` (dotted) in `value` to `next`, creating the tables on the way.
/// A document or a table on the way that holds something other than an
/// object is refused naming it, never replaced by an empty one, which would
/// throw away whatever it held.
pub(crate) fn config_set_value(value: &mut Value, key: &str, next: Value) -> Result<(), String> {
    let parts = key
        .split('.')
        .filter(|part| !part.is_empty())
        .collect::<Vec<_>>();
    let Some((last, prefix)) = parts.split_last() else {
        return Err("config key is required".into());
    };
    let mut current = value
        .as_object_mut()
        .ok_or_else(|| format!("the configuration document is not an object, so {key} is not set"))?;
    for (depth, part) in prefix.iter().enumerate() {
        let table = current
            .entry((*part).to_string())
            .or_insert_with(|| json!({}));
        current = match table {
            Value::Object(table) => table,
            other => {
                return Err(format!(
                    "{} holds {other}, not a table, so {key} is not set",
                    prefix[..=depth].join(".")
                ))
            }
        };
    }
    current.insert((*last).to_string(), next);
    Ok(())
}

/// Remove one declared key from a config document, and any object the
/// removal leaves empty.
///
/// `config reset` writes the schema default, which is not the same thing:
/// the file keeps saying something about the key. A key that was never in
/// the file has no way back to absent without this, which is what a test —
/// or an operator undoing an experiment — needs. Returns whether anything
/// was there to remove.
pub(crate) fn config_remove_value(value: &mut Value, key: &str) -> Result<bool, String> {
    let parts = key
        .split('.')
        .filter(|part| !part.is_empty())
        .collect::<Vec<_>>();
    let Some((last, prefix)) = parts.split_last() else {
        return Err("config key is required".into());
    };
    let mut current = &mut *value;
    for part in prefix {
        let Some(next) = current.get_mut(*part) else {
            return Ok(false);
        };
        current = next;
    }
    let Some(object) = current.as_object_mut() else {
        return Ok(false);
    };
    let removed = object.remove(*last).is_some();
    if removed {
        prune_empty_objects(value, prefix);
    }
    Ok(removed)
}

/// Drop the objects the removal emptied, outermost last, so a file does not
/// keep `"ui": {}` after its only setting is gone.
fn prune_empty_objects(value: &mut Value, prefix: &[&str]) {
    for (depth, name) in prefix.iter().enumerate().rev() {
        let mut current = &mut *value;
        for part in &prefix[..depth] {
            let Some(next) = current.get_mut(*part) else {
                return;
            };
            current = next;
        }
        let Some(object) = current.as_object_mut() else {
            return;
        };
        let empty = object
            .get(*name)
            .and_then(Value::as_object)
            .is_some_and(serde_json::Map::is_empty);
        if !empty {
            return;
        }
        object.remove(*name);
    }
}
