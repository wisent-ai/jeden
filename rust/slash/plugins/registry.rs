use serde_json::{json, Value};
use std::path::{Path, PathBuf};

use crate::slash::common::{now_text, read_json_value, write_json_value};

pub(crate) fn plugin_registry_path(cwd: &Path) -> PathBuf {
    cwd.join(".jeden/plugins.json")
}

/// The plugin registry under `cwd`, normalised: absent is empty; one that
/// cannot be read, does not parse, or whose root, `sources`, `installed` or
/// `reload` is not an object is refused naming the file and the field. Read
/// as empty, every installed plugin vanished from the list, and the next save
/// wrote that empty list over the file.
pub(crate) fn plugin_registry(cwd: &Path) -> Result<Value, String> {
    let path = plugin_registry_path(cwd);
    let raw = read_json_value(&path)?;
    let root = raw
        .as_object()
        .ok_or_else(|| format!("{} does not hold a JSON object", path.display()))?;
    // `null` is what this registry writes for a field that holds nothing
    // (`reload` after no reload), so it reads back as that field's absence.
    let field = |name: &str, absent: Value| match root.get(name) {
        None | Some(Value::Null) => Ok(absent),
        Some(value) if value.is_object() => Ok(value.clone()),
        Some(other) => Err(format!(
            "{}: {name} must be an object, not {other}",
            path.display()
        )),
    };
    Ok(json!({
        "sources": field("sources", json!({}))?,
        "installed": field("installed", json!({}))?,
        "reload": field("reload", Value::Null)?,
    }))
}

pub(crate) fn save_plugin_registry(cwd: &Path, registry: &Value) -> Result<PathBuf, String> {
    let file = plugin_registry_path(cwd);
    let mut normalized = plugin_registry(cwd)?;
    if let Some(map) = registry.as_object() {
        if let Some(sources) = map.get("sources").filter(|value| value.is_object()) {
            normalized["sources"] = sources.clone();
        }
        if let Some(installed) = map.get("installed").filter(|value| value.is_object()) {
            normalized["installed"] = installed.clone();
        }
        if let Some(reload) = map.get("reload") {
            normalized["reload"] = reload.clone();
        }
    }
    normalized["updatedAt"] = json!(now_text());
    write_json_value(&file, &normalized)?;
    Ok(file)
}

/// A field of a registry entry, or the field named as missing: a dash read
/// as a plugin, type or source called "-".
fn named(value: &Value, field: &str) -> String {
    match value.get(field).and_then(Value::as_str) {
        Some(text) => text.to_string(),
        None => format!("({field} not recorded)"),
    }
}

pub(crate) fn format_plugin_source(value: &Value) -> String {
    format!(
        "{}\t{}\t{}\t{}",
        named(value, "name"),
        named(value, "type"),
        named(value, "source"),
        if value.get("enabled").and_then(Value::as_bool) == Some(false) {
            "disabled"
        } else {
            "enabled"
        }
    )
}

pub(crate) fn format_plugin(value: &Value) -> String {
    format!(
        "{}\t{}\t{}\t{}",
        named(value, "id"),
        named(value, "version"),
        if value.get("enabled").and_then(Value::as_bool) == Some(false) {
            "disabled"
        } else {
            "enabled"
        },
        named(value, "source")
    )
}

pub(crate) fn sorted_object_values(value: &Value) -> Vec<Value> {
    let mut values = value
        .as_object()
        .map(|map| map.values().cloned().collect::<Vec<_>>())
        .unwrap_or_default();
    values.sort_by_key(format_plugin);
    values
}
