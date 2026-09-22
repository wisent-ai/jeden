//! Turning what an operator typed into the value a setting actually takes,
//! and refusing what does not fit.
//!
//! Split out of `cli/config/schema.rs`, which had grown past the module line
//! cap.

use super::super::config_value_at;
use super::table::SETTINGS_SCHEMA;
use super::SettingSpec;
use serde_json::{json, Value};
use crate::cli::config::parse_config_literal;

pub(super) fn setting_spec(key: &str) -> Option<&'static SettingSpec> {
    SETTINGS_SCHEMA.iter().find(|spec| spec.key == key)
}

pub(crate) fn setting_default(spec: &SettingSpec) -> Value {
    serde_json::from_str(spec.default_json).unwrap_or(Value::Null)
}

pub(super) fn effective_setting_value(config: &Value, spec: &SettingSpec) -> Value {
    config_value_at(config, spec.key)
        .cloned()
        .unwrap_or_else(|| setting_default(spec))
}

pub(super) fn parse_setting_value(spec: &SettingSpec, raw: &str) -> Result<Value, String> {
    let trimmed = raw.trim();
    match spec.typ {
        "boolean" => match trimmed.to_ascii_lowercase().as_str() {
            "true" | "yes" | "on" | "1" => Ok(json!(true)),
            "false" | "no" | "off" | "0" => Ok(json!(false)),
            _ => Err(format!(
                "{} expects a boolean (true/false, yes/no, on/off, 1/0)",
                spec.key
            )),
        },
        "number" => {
            let number = trimmed
                .parse::<f64>()
                .map_err(|_| format!("{} expects a finite number", spec.key))?;
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
        "string" => Ok(json!(trimmed)),
        _ => Ok(parse_config_literal(trimmed)),
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
        out.as_object_mut()
            .expect("object")
            .insert("enum".into(), json!(spec.enum_values));
    }
    out
}
