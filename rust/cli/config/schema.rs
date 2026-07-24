//! Settings schema, per-key parse and metadata, list rendering, and the config subcommand.

use serde_json::{json, Value};
use std::path::Path;

use super::{
    config_set_value, config_value_at, merged_config_value, parse_config_literal,
    read_user_writable_config, write_user_config,
};
use crate::tui::{PickerItem, PickerSpec};
use crate::user_config_path;
use crate::Args;

#[derive(Clone, Copy)]
pub(crate) struct SettingSpec {
    pub(crate) key: &'static str,
    pub(crate) typ: &'static str,
    pub(crate) description: &'static str,
    pub(crate) default_json: &'static str,
    pub(crate) enum_values: &'static [&'static str],
}

pub(crate) const SETTINGS_SCHEMA: &[SettingSpec] = &[
    SettingSpec {
        key: "tools.approvalMode",
        typ: "enum",
        description: "Default approval policy for tool execution.",
        default_json: "\"always-ask\"",
        enum_values: &["always-ask", "write", "yolo"],
    },
    SettingSpec {
        key: "commands.enableClaudeUser",
        typ: "boolean",
        description: "Enable user slash commands from ~/.claude/commands.",
        default_json: "true",
        enum_values: &[],
    },
    SettingSpec {
        key: "commands.enableClaudeProject",
        typ: "boolean",
        description: "Enable project slash commands from .claude/commands.",
        default_json: "true",
        enum_values: &[],
    },
    SettingSpec {
        key: "commands.enableOpencodeUser",
        typ: "boolean",
        description: "Enable user slash commands from ~/.config/opencode/commands.",
        default_json: "true",
        enum_values: &[],
    },
    SettingSpec {
        key: "commands.enableOpencodeProject",
        typ: "boolean",
        description: "Enable project slash commands from .opencode/commands.",
        default_json: "true",
        enum_values: &[],
    },
    SettingSpec {
        key: "startup.showSplash",
        typ: "boolean",
        description: "Show the startup splash animation on normal launches.",
        default_json: "false",
        enum_values: &[],
    },
    SettingSpec {
        key: "startup.quiet",
        typ: "boolean",
        description: "Suppress startup chrome including the splash.",
        default_json: "false",
        enum_values: &[],
    },
    SettingSpec {
        key: "context.maxBytes",
        typ: "number",
        description: "Maximum UTF-8 bytes loaded from discovered context and rule files.",
        default_json: "131072",
        enum_values: &[],
    },
    SettingSpec {
        key: "context.maxTokens",
        typ: "number",
        description: "Approximate token budget for discovered context and rule files.",
        default_json: "32768",
        enum_values: &[],
    },
    SettingSpec {
        key: "rules.alwaysApply",
        typ: "array",
        description: "Typed sticky rules injected into every rebuilt system prompt.",
        default_json: "[]",
        enum_values: &[],
    },
    SettingSpec {
        key: "secrets.mode",
        typ: "enum",
        description: "Protect known secrets in model-bound text by redaction or obfuscation.",
        default_json: "\"redact\"",
        enum_values: &["redact", "obfuscate"],
    },
    SettingSpec {
        key: "secrets.minLength",
        typ: "number",
        description: "Minimum length for automatically discovered environment secrets.",
        default_json: "8",
        enum_values: &[],
    },
    SettingSpec {
        key: "secrets.discoverEnvironment",
        typ: "boolean",
        description: "Automatically protect values from secret-named environment variables.",
        default_json: "true",
        enum_values: &[],
    },
    SettingSpec {
        key: "ui.language",
        typ: "enum",
        description: "Conversation language: auto follows the user's messages; an ISO 639 code pins the answer language (65 languages as in wisent-app).",
        default_json: "\"auto\"",
        enum_values: &[
            "auto", "am", "ar", "az", "be", "bg", "bn", "bs", "ca", "cs", "da", "de", "dv", "dz",
            "el", "en", "es", "et", "fa", "fi", "fo", "fr", "he", "hr", "hu", "hy", "id", "is",
            "it", "ja", "ka", "kk", "kl", "km", "ko", "ky", "lo", "lt", "lv", "mk", "mn", "ms",
            "my", "ne", "nl", "no", "pl", "ps", "pt", "ro", "ru", "si", "sk", "sl", "so", "sq",
            "sr", "sv", "tg", "th", "tk", "tr", "uk", "uz", "vi", "zh",
        ],
    },
];

fn setting_spec(key: &str) -> Option<&'static SettingSpec> {
    SETTINGS_SCHEMA.iter().find(|spec| spec.key == key)
}

fn setting_default(spec: &SettingSpec) -> Value {
    serde_json::from_str(spec.default_json).unwrap_or(Value::Null)
}

fn effective_setting_value(config: &Value, spec: &SettingSpec) -> Value {
    config_value_at(config, spec.key)
        .cloned()
        .unwrap_or_else(|| setting_default(spec))
}

fn parse_setting_value(spec: &SettingSpec, raw: &str) -> Result<Value, String> {
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
            if spec.enum_values.iter().any(|value| *value == trimmed) {
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

fn setting_metadata(spec: &SettingSpec, value: Value) -> Value {
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

fn config_list_json(cwd: &Path) -> Value {
    let config = merged_config_value(cwd);
    let mut out = serde_json::Map::new();
    for spec in SETTINGS_SCHEMA {
        out.insert(
            spec.key.to_string(),
            setting_metadata(spec, effective_setting_value(&config, spec)),
        );
    }
    Value::Object(out)
}

fn config_list_text(cwd: &Path) -> String {
    let list = config_list_json(cwd);
    let lang = crate::cli::i18n::lang_code(cwd);
    let mut lines = vec![
        crate::cli::i18n::tr(&lang, "view.settings.title").to_string(),
        format!("Config: {}", user_config_path().display()),
    ];
    let mut current_group = "";
    for spec in SETTINGS_SCHEMA {
        let group = spec.key.split('.').next().unwrap_or("settings");
        if group != current_group {
            current_group = group;
            lines.push(format!("\n[{group}]"));
        }
        let value = &list[spec.key]["value"];
        let value_text = value
            .as_str()
            .map(str::to_string)
            .unwrap_or_else(|| value.to_string());
        lines.push(format!(
            "{} = {} ({}) - {}",
            spec.key, value_text, spec.typ, spec.description
        ));
    }
    lines.join("\n") + "\n"
}

/// Non-action group header; the picker skips disabled, command-less rows.
/// Same pattern as the models picker `group_header` in cli/run/slash_ui.rs.
fn section_header(prefix: &str, count: usize) -> PickerItem {
    let mut header = PickerItem::action(format!("── {prefix} ({count}) ──"), "");
    header.command = None;
    header.disabled = true;
    header
}

/// Group rows by top-level key prefix: known prefixes in first-seen order,
/// anything else under `other` after them. Row order within a group is kept.
fn grouped_setting_rows(rows: Vec<(&str, PickerItem)>) -> Vec<PickerItem> {
    const KNOWN_PREFIXES: &[&str] = &["tools", "commands", "startup", "context", "rules", "secrets", "ui"];
    let mut groups: Vec<(&str, Vec<PickerItem>)> = Vec::new();
    for (prefix, item) in rows {
        let label = if KNOWN_PREFIXES.contains(&prefix) {
            prefix
        } else {
            "other"
        };
        match groups.iter_mut().find(|(name, _)| *name == label) {
            Some((_, items)) => items.push(item),
            None => groups.push((label, vec![item])),
        }
    }
    groups.sort_by_key(|(name, _)| usize::from(*name == "other"));
    let mut items = Vec::new();
    for (name, group_rows) in groups {
        items.push(section_header(name, group_rows.len()));
        items.extend(group_rows);
    }
    items
}

pub(crate) fn settings_picker(cwd: &Path) -> PickerSpec {
    let (config, mut rows) = (merged_config_value(cwd), Vec::new());
    let lang = crate::cli::i18n::lang_code(cwd);
    for spec in SETTINGS_SCHEMA {
        let (current, default) = (
            effective_setting_value(&config, spec),
            setting_default(spec),
        );
        let current_text = current
            .as_str()
            .map(str::to_string)
            .unwrap_or_else(|| current.to_string());
        let detail = format!("{} Current: {current_text}.", spec.description);
        let prefix = spec.key.split('.').next().unwrap_or("other");
        match spec.typ {
            "boolean" => {
                let next = !current.as_bool().unwrap_or(false);
                rows.push((
                    prefix,
                    PickerItem::action(
                        format!("{}: set {next}", spec.key),
                        format!("/settings set {} {next}", spec.key),
                    )
                    .detail(&detail)
                    .badge(current_text.to_ascii_uppercase()),
                ));
            }
            "enum" => {
                for value in spec.enum_values {
                    let active = current.as_str() == Some(*value);
                    let item = PickerItem::action(
                        format!("{}: {value}", spec.key),
                        format!("/settings set {} {value}", spec.key),
                    )
                    .detail(&detail)
                    .disabled(active);
                    rows.push((
                        prefix,
                        if active {
                            item.badge(crate::cli::i18n::tr(&lang, "badge.active"))
                        } else {
                            item
                        },
                    ));
                }
            }
            _ => {}
        }
        if current != default {
            rows.push((
                prefix,
                PickerItem::action(
                    format!("{}: reset to default", spec.key),
                    format!("/settings reset {}", spec.key),
                )
                .detail(format!(
                    "{} Current: {current_text}. Default: {default}.",
                    spec.description
                ))
                .badge("RESET"),
            ));
        }
    }
    PickerSpec::new(
        crate::cli::i18n::tr(&lang, "view.settings.title"),
        grouped_setting_rows(rows),
    )
    .localized(&lang)
}
pub(crate) fn config_command(args: &Args) -> Result<String, String> {
    let (verb, rest) = args
        .positionals
        .split_first()
        .map(|(v, r)| (v.as_str(), r))
        .unwrap_or(("list", &[]));
    match verb {
        "list" => Ok(if args.json {
            serde_json::to_string_pretty(&config_list_json(&args.cwd))
                .map_err(|error| error.to_string())?
                + "\n"
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
                serde_json::to_string_pretty(&setting_metadata(spec, value))
                    .map_err(|error| error.to_string())?
                    + "\n"
            } else if let Some(text) = value.as_str() {
                format!("{text}\n")
            } else {
                serde_json::to_string_pretty(&value).map_err(|error| error.to_string())? + "\n"
            })
        }
        "set" => {
            let (key, value_tokens) = rest.split_first().ok_or("config set requires a key")?;
            let spec = setting_spec(key).ok_or_else(|| format!("unknown config key: {key}"))?;
            if value_tokens.is_empty() {
                return Err("config set requires a value".into());
            }
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
        _ => Err(
            "Usage: jeden config [list|path|get <key>|set <key> <value>|reset <key>] [--json]"
                .into(),
        ),
    }
}
