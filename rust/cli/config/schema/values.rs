//! Turning what an operator typed into the value a setting actually takes,
//! and refusing what does not fit.
//!
//! Split out of `cli/config/schema.rs`, which had grown past the module line
//! cap.

use super::super::config_value_at;
use super::table::SETTINGS_SCHEMA;
use super::SettingSpec;
use serde_json::{json, Value};

pub(super) fn setting_spec(key: &str) -> Option<&'static SettingSpec> {
    SETTINGS_SCHEMA.iter().find(|spec| spec.key == key)
}

/// A setting's default as the compiled schema states it; a default that is
/// not JSON is a defect in the schema table, not in anyone's configuration.
pub(crate) fn setting_default(spec: &SettingSpec) -> Value {
    serde_json::from_str(spec.default_json)
        .expect("every default in the compiled settings schema is JSON")
}

/// The value configured for the setting, or its default when none is.
pub(super) fn effective_setting_value(config: &Value, spec: &SettingSpec) -> Value {
    match config_value_at(config, spec.key) {
        Some(value) => value.clone(),
        None => setting_default(spec),
    }
}

pub(super) fn parse_setting_value(spec: &SettingSpec, raw: &str) -> Result<Value, String> {
    let trimmed = raw.trim();
    match spec.typ {
        "boolean" => trimmed
            .to_ascii_lowercase()
            .parse::<bool>()
            .map(|value| json!(value))
            .map_err(|error| format!("{} expects true or false: {error}", spec.key)),
        "number" if spec.key == "secrets.minLength" => trimmed
            .parse::<usize>()
            .map(|minimum| json!(minimum))
            .map_err(|error| {
                format!(
                    "{} expects a nonnegative whole byte length: {error}",
                    spec.key
                )
            }),
        "number" => {
            let number = trimmed
                .parse::<f64>()
                .map_err(|error| format!("{} expects a finite number: {error}", spec.key))?;
            if number.is_finite() {
                Ok(json!(number))
            } else {
                Err(format!("{} expects a finite number", spec.key))
            }
        }
        "enum" => {
            if spec.enum_values.contains(&trimmed) {
                Ok(json!(trimmed))
            } else {
                Err(format!(
                    "{} must be one of: {}",
                    spec.key,
                    spec.enum_values.join(", ")
                ))
            }
        }
        "array" => {
            let value = serde_json::from_str::<Value>(trimmed)
                .map_err(|error| format!("{} expects a JSON array: {error}", spec.key))?;
            if value.is_array() {
                Ok(value)
            } else {
                Err(format!("{} expects a JSON array", spec.key))
            }
        }
        "record" => {
            let value = serde_json::from_str::<Value>(trimmed)
                .map_err(|error| format!("{} expects a JSON object: {error}", spec.key))?;
            if value.as_object().is_some() {
                Ok(value)
            } else {
                Err(format!("{} expects a JSON object", spec.key))
            }
        }
        "task-limits" => {
            let value = serde_json::from_str::<Value>(trimmed)
                .map_err(|error| format!("{} expects a JSON object: {error}", spec.key))?;
            crate::task_runtime::check_declaration(&value)?;
            Ok(value)
        }
        "string" => Ok(json!(trimmed)),
        // A schema type this parser does not read is refused naming it, never
        // guessed from the text as a boolean, number or JSON.
        other => Err(format!(
            "{} has the schema type {other}, which `jeden config set` cannot read",
            spec.key
        )),
    }
}

pub(crate) fn setting_metadata(spec: &SettingSpec, value: Value) -> Value {
    let mut out = json!({
        "value": value,
        "type": spec.typ,
        "description": spec.description,
        "default": setting_default(spec),
    });
    if !spec.enum_values.is_empty() {
        out["enum"] = json!(spec.enum_values);
    }
    out
}
