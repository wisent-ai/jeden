//! Settings schema, per-key parse and metadata, list rendering, and the config subcommand.

use serde_json::{json, Value};
use std::path::Path;

use crate::Args;
use crate::user_config_path;
use super::{config_set_value, config_value_at, merged_config_value, parse_config_literal, read_user_writable_config, write_user_config};

#[derive(Clone, Copy)]
pub(crate) struct SettingSpec {
    pub(crate) key: &'static str,
    pub(crate) typ: &'static str,
    pub(crate) description: &'static str,
    pub(crate) default_json: &'static str,
    pub(crate) enum_values: &'static [&'static str],
}

pub(crate) const SETTINGS_SCHEMA: &[SettingSpec] = &[
    SettingSpec { key: "tools.approvalMode", typ: "enum", description: "Default approval policy for tool execution.", default_json: "\"always-ask\"", enum_values: &["always-ask", "write", "yolo"] },
    SettingSpec { key: "commands.enableClaudeUser", typ: "boolean", description: "Enable user slash commands from ~/.claude/commands.", default_json: "true", enum_values: &[] },
    SettingSpec { key: "commands.enableClaudeProject", typ: "boolean", description: "Enable project slash commands from .claude/commands.", default_json: "true", enum_values: &[] },
    SettingSpec { key: "commands.enableOpencodeUser", typ: "boolean", description: "Enable user slash commands from ~/.config/opencode/commands.", default_json: "true", enum_values: &[] },
    SettingSpec { key: "commands.enableOpencodeProject", typ: "boolean", description: "Enable project slash commands from .opencode/commands.", default_json: "true", enum_values: &[] },
    SettingSpec { key: "startup.showSplash", typ: "boolean", description: "Show the startup splash animation on normal launches.", default_json: "false", enum_values: &[] },
    SettingSpec { key: "startup.quiet", typ: "boolean", description: "Suppress startup chrome including the splash.", default_json: "false", enum_values: &[] },
];

fn setting_spec(key: &str) -> Option<&'static SettingSpec> {
    SETTINGS_SCHEMA.iter().find(|spec| spec.key == key)
}

fn setting_default(spec: &SettingSpec) -> Value {
    serde_json::from_str(spec.default_json).unwrap_or(Value::Null)
}

fn effective_setting_value(config: &Value, spec: &SettingSpec) -> Value {
    config_value_at(config, spec.key).cloned().unwrap_or_else(|| setting_default(spec))
}

fn parse_setting_value(spec: &SettingSpec, raw: &str) -> Result<Value, String> {
    let trimmed = raw.trim();
    match spec.typ {
        "boolean" => match trimmed.to_ascii_lowercase().as_str() {
            "true" | "yes" | "on" | "1" => Ok(json!(true)),
            "false" | "no" | "off" | "0" => Ok(json!(false)),
            _ => Err(format!("{} expects a boolean (true/false, yes/no, on/off, 1/0)", spec.key)),
        },
        "number" => {
            let number = trimmed.parse::<f64>().map_err(|_| format!("{} expects a finite number", spec.key))?;
            if number.is_finite() { Ok(json!(number)) } else { Err(format!("{} expects a finite number", spec.key)) }
        }
        "enum" => {
            if spec.enum_values.iter().any(|value| *value == trimmed) {
                Ok(json!(trimmed))
            } else {
                Err(format!("{} must be one of: {}", spec.key, spec.enum_values.join(", ")))
            }
        }
        "array" => {
            let value = serde_json::from_str::<Value>(trimmed).map_err(|error| format!("{} expects a JSON array: {error}", spec.key))?;
            if value.is_array() { Ok(value) } else { Err(format!("{} expects a JSON array", spec.key)) }
        }
        "record" => {
            let value = serde_json::from_str::<Value>(trimmed).map_err(|error| format!("{} expects a JSON object: {error}", spec.key))?;
            if value.as_object().is_some() { Ok(value) } else { Err(format!("{} expects a JSON object", spec.key)) }
        }
        "string" => Ok(json!(trimmed)),
        _ => Ok(parse_config_literal(trimmed)),
    }
}

fn setting_metadata(spec: &SettingSpec, value: Value) -> Value {
    let mut out = json!({
        "value": value,
        "type": spec.typ,
        "description": spec.description,
        "default": setting_default(spec),
    });
    if !spec.enum_values.is_empty() {
        out.as_object_mut().expect("object").insert("enum".into(), json!(spec.enum_values));
    }
    out
}

fn config_list_json(cwd: &Path) -> Value {
    let config = merged_config_value(cwd);
    let mut out = serde_json::Map::new();
    for spec in SETTINGS_SCHEMA {
        out.insert(spec.key.to_string(), setting_metadata(spec, effective_setting_value(&config, spec)));
    }
    Value::Object(out)
}

fn config_list_text(cwd: &Path) -> String {
    let list = config_list_json(cwd);
    let mut lines = vec!["Jeden settings".to_string(), format!("Config: {}", user_config_path().display())];
    let mut current_group = "";
    for spec in SETTINGS_SCHEMA {
        let group = spec.key.split('.').next().unwrap_or("settings");
        if group != current_group {
            current_group = group;
            lines.push(format!("\n[{group}]"));
        }
        let value = &list[spec.key]["value"];
        let value_text = value.as_str().map(str::to_string).unwrap_or_else(|| value.to_string());
        lines.push(format!("{} = {} ({}) - {}", spec.key, value_text, spec.typ, spec.description));
    }
    lines.join("\n") + "\n"
}

pub(crate) fn config_command(args: &Args) -> Result<String, String> {
    let (verb, rest) = args.positionals.split_first().map(|(v, r)| (v.as_str(), r)).unwrap_or(("list", &[]));
    match verb {
        "list" => Ok(if args.json {
            serde_json::to_string_pretty(&config_list_json(&args.cwd)).map_err(|error| error.to_string())? + "\n"
        } else {
            config_list_text(&args.cwd)
        }),
        "path" => Ok(format!("{}\n", user_config_path().display())),
        "get" => {
            let key = rest.first().ok_or("config get requires a key")?;
            let spec = setting_spec(key).ok_or_else(|| format!("unknown config key: {key}"))?;
            let config = merged_config_value(&args.cwd);
            let value = effective_setting_value(&config, spec);
            Ok(if args.json {
                serde_json::to_string_pretty(&setting_metadata(spec, value)).map_err(|error| error.to_string())? + "\n"
            } else if let Some(text) = value.as_str() {
                format!("{text}\n")
            } else {
                serde_json::to_string_pretty(&value).map_err(|error| error.to_string())? + "\n"
            })
        }
        "set" => {
            let (key, value_tokens) = rest.split_first().ok_or("config set requires a key")?;
            let spec = setting_spec(key).ok_or_else(|| format!("unknown config key: {key}"))?;
            if value_tokens.is_empty() { return Err("config set requires a value".into()); }
            let raw = value_tokens.join(" ");
            let parsed = parse_setting_value(spec, &raw)?;
            let mut config = read_user_writable_config();
            config_set_value(&mut config, key, parsed.clone())?;
            let path = write_user_config(&config)?;
            Ok(if args.json {
                serde_json::to_string_pretty(&json!({"key": key, "value": parsed, "type": spec.typ, "description": spec.description, "path": path})).map_err(|error| error.to_string())? + "\n"
            } else {
                format!("Set {key} in {}\n", path.display())
            })
        }
        "reset" => {
            let key = rest.first().ok_or("config reset requires a key")?;
            let spec = setting_spec(key).ok_or_else(|| format!("unknown config key: {key}"))?;
            let default_value = setting_default(spec);
            let mut config = read_user_writable_config();
            config_set_value(&mut config, key, default_value.clone())?;
            let path = write_user_config(&config)?;
            Ok(if args.json {
                serde_json::to_string_pretty(&json!({"key": key, "value": default_value, "type": spec.typ, "description": spec.description, "path": path})).map_err(|error| error.to_string())? + "\n"
            } else {
                format!("Reset {key} to schema default in {}\n", path.display())
            })
        }
        _ => Err("Usage: jeden config [list|path|get <key>|set <key> <value>|reset <key>] [--json]".into()),
    }
}
