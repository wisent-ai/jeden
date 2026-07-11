use serde_json::{json, Value};
use std::path::{Path, PathBuf};

use crate::slash::common::{now_text, read_json_value, write_json_value};

pub(crate) fn plugin_registry_path(cwd: &Path) -> PathBuf {
    cwd.join(".jeden/plugins.json")
}

pub(crate) fn plugin_registry(cwd: &Path) -> Value {
    let raw = read_json_value(&plugin_registry_path(cwd));
    json!({
        "sources": raw.get("sources").filter(|value| value.is_object()).cloned().unwrap_or_else(|| json!({})),
        "installed": raw.get("installed").filter(|value| value.is_object()).cloned().unwrap_or_else(|| json!({})),
        "reload": raw.get("reload").filter(|value| value.is_object()).cloned().unwrap_or(Value::Null),
    })
}

pub(crate) fn save_plugin_registry(cwd: &Path, registry: &Value) -> Result<PathBuf, String> {
    let file = plugin_registry_path(cwd);
    let mut normalized = plugin_registry(cwd);
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

pub(crate) fn format_plugin_source(value: &Value) -> String {
    format!(
        "{}\t{}\t{}\t{}",
        value.get("name").and_then(Value::as_str).unwrap_or("-"),
        value.get("type").and_then(Value::as_str).unwrap_or("-"),
        value.get("source").and_then(Value::as_str).unwrap_or("-"),
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
        value.get("id").and_then(Value::as_str).unwrap_or("-"),
        value.get("version").and_then(Value::as_str).unwrap_or("-"),
        if value.get("enabled").and_then(Value::as_bool) == Some(false) {
            "disabled"
        } else {
            "enabled"
        },
        value.get("source").and_then(Value::as_str).unwrap_or("-")
    )
}

pub(crate) fn sorted_object_values(value: &Value) -> Vec<Value> {
    let mut values = value
        .as_object()
        .map(|map| map.values().cloned().collect::<Vec<_>>())
        .unwrap_or_default();
    values.sort_by(|a, b| format_plugin(a).cmp(&format_plugin(b)));
    values
}
