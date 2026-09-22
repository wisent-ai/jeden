//! Finding the configuration files that apply here, and merging them in the
//! right order.
//!
//! Split out of `cli/config/mod.rs`, which had grown past the module line cap.

use crate::{config_path, legacy_user_config_path, user_config_path};
use serde_json::{json, Value};
use std::fs;
use std::path::{Path, PathBuf};
use serde::Deserialize;

pub(crate) fn read_config_value(path: &Path) -> Value {
    let Some(text) = fs::read_to_string(path).ok() else {
        return json!({});
    };
    let parsed = match path
        .extension()
        .and_then(|ext| ext.to_str())
        .unwrap_or("")
        .to_ascii_lowercase()
        .as_str()
    {
        "yml" | "yaml" => serde_yaml::from_str::<Value>(&text).ok(),
        _ => serde_json::from_str::<Value>(&text).ok(),
    };
    parsed.filter(Value::is_object).unwrap_or_else(|| json!({}))
}

pub(crate) fn read_config_typed<T: for<'a> Deserialize<'a> + Default>(path: &Path) -> T {
    serde_json::from_value(read_config_value(path)).unwrap_or_default()
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

pub(crate) fn config_set_value(value: &mut Value, key: &str, next: Value) -> Result<(), String> {
    if !value.is_object() {
        *value = json!({});
    }
    let parts = key
        .split('.')
        .filter(|part| !part.is_empty())
        .collect::<Vec<_>>();
    let Some((last, prefix)) = parts.split_last() else {
        return Err("config key is required".into());
    };
    let mut current = value;
    for part in prefix {
        if !current.get(*part).map(Value::is_object).unwrap_or(false) {
            current
                .as_object_mut()
                .expect("object")
                .insert((*part).to_string(), json!({}));
        }
        current = current.get_mut(*part).expect("inserted object");
    }
    current
        .as_object_mut()
        .expect("object")
        .insert((*last).to_string(), next);
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
        prune_empty_objects(value, &parts[..parts.len() - 1]);
    }
    Ok(removed)
}

/// Drop the objects the removal emptied, outermost last, so a file does not
/// keep `"ui": {}` after its only setting is gone.
fn prune_empty_objects(value: &mut Value, prefix: &[&str]) {
    for depth in (0..prefix.len()).rev() {
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
            .get(prefix[depth])
            .and_then(Value::as_object)
            .is_some_and(serde_json::Map::is_empty);
        if !empty {
            return;
        }
        object.remove(prefix[depth]);
    }
}

pub(crate) fn parse_config_literal(raw: &str) -> Value {
    let trimmed = raw.trim();
    if trimmed.eq_ignore_ascii_case("true") {
        return json!(true);
    }
    if trimmed.eq_ignore_ascii_case("false") {
        return json!(false);
    }
    if let Ok(number) = trimmed.parse::<f64>() {
        if number.is_finite() {
            return json!(number);
        }
    }
    serde_json::from_str::<Value>(trimmed).unwrap_or_else(|_| json!(trimmed))
}
