//! Settings schema, per-key parse and metadata, list rendering, and the config subcommand.

use serde_json::{json, Value};
use std::path::Path;

use super::communication::{CommunicationMode, DisplayPolicy, Visibility};
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

pub(crate) const COMMUNICATION_CONTRACT_KEY: &str = "contracts.communication";
pub(crate) const FUNCTIONALITY_CONTRACT_KEY: &str = "contracts.functionality";
pub(crate) const COMMUNICATION_MODE_KEY: &str = "communication.mode";
pub(crate) const COMMUNICATION_TOOL_CALLS_KEY: &str = "communication.toolCalls";
pub(crate) const COMMUNICATION_TOOL_RESULTS_KEY: &str = "communication.toolResults";
pub(crate) const COMMUNICATION_REASONING_KEY: &str = "communication.reasoning";
pub(crate) const COMMUNICATION_CODE_KEY: &str = "communication.code";

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
        key: COMMUNICATION_CONTRACT_KEY,
        typ: "string",
        description: "Instructions for how Jeden communicates: language, tone, length, structure, and terminology.",
        default_json: "\"\"",
        enum_values: &[],
    },
    SettingSpec {
        key: FUNCTIONALITY_CONTRACT_KEY,
        typ: "string",
        description: "Instructions for how Jeden carries out work and what it must complete before answering.",
        default_json: "\"\"",
        enum_values: &[],
    },
    SettingSpec {
        key: COMMUNICATION_MODE_KEY,
        typ: "enum",
        description: "Communication mode: normal shows tool names while working, debug also shows each tool call with its input, every tool result, and the model's reasoning, quiet shows only the answer.",
        default_json: "\"normal\"",
        enum_values: CommunicationMode::VALUES,
    },
    SettingSpec {
        key: COMMUNICATION_TOOL_CALLS_KEY,
        typ: "enum",
        description: "Show the model's tool calls; auto follows the mode.",
        default_json: "\"auto\"",
        enum_values: Visibility::VALUES,
    },
    SettingSpec {
        key: COMMUNICATION_TOOL_RESULTS_KEY,
        typ: "enum",
        description: "Show what each tool returned; auto follows the mode.",
        default_json: "\"auto\"",
        enum_values: Visibility::VALUES,
    },
    SettingSpec {
        key: COMMUNICATION_REASONING_KEY,
        typ: "enum",
        description: "Show the model's reasoning when the route streams it; auto follows the mode.",
        default_json: "\"auto\"",
        enum_values: Visibility::VALUES,
    },
    SettingSpec {
        key: COMMUNICATION_CODE_KEY,
        typ: "enum",
        description: "Show code blocks in answers; hide replaces each block with a placeholder and asks the model to answer in prose. Auto follows the mode.",
        default_json: "\"auto\"",
        enum_values: Visibility::VALUES,
    },
    SettingSpec {
        key: "rules.alwaysApply",
        typ: "array",
        description: "Typed sticky rules injected into every rebuilt system prompt.",
        default_json: "[]",
        enum_values: &[],
    },
    SettingSpec {
        key: "hooks.tamaRegistry",
        typ: "string",
        description: "Path to the Tama hook registry (shared-hooks registry.json). Empty disables Tama hooks; unset auto-discovers known locations.",
        default_json: "\"\"",
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
    SettingSpec {
        key: "ui.theme",
        typ: "enum",
        description: "Color theme: a named preset, 'custom' to load .jeden/theme.json, or 'auto' (graphite-dark).",
        default_json: "\"auto\"",
        enum_values: &[
            "auto",
            "graphite-dark",
            "paper-light",
            "titanium",
            "nord",
            "color-blind",
            "mono",
            "high-contrast",
            "custom",
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

/// Group rows by top-level key prefix into picker tabs: known prefixes in
/// first-seen order, anything else under `other` after them. Row order within
/// a group is kept. Returns the tab bar (index 0 = the catch-all "All" view)
/// and the rows tagged with their 1-based tab index.
fn grouped_setting_rows(
    rows: Vec<(&str, PickerItem)>,
    lang: &str,
) -> (Vec<String>, Vec<PickerItem>) {
    const KNOWN_PREFIXES: &[&str] = &[
        "tools",
        "commands",
        "startup",
        "context",
        "contracts",
        "communication",
        "rules",
        "hooks",
        "secrets",
        "ui",
    ];
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
    let mut tabs = vec![crate::cli::i18n::tr(lang, "picker.tab.all").to_string()];
    let mut items = Vec::new();
    for (name, group_rows) in groups {
        tabs.push(name.to_string());
        let tab = tabs.len() - 1;
        items.extend(group_rows.into_iter().map(|item| item.tab(tab)));
    }
    (tabs, items)
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
            _ => {
                // Number/string/array/record keys have no enumerable values:
                // offer a prefill row that drops `/settings set <key> ` into
                // the prompt so every schema key is editable from the picker.
                rows.push((
                    prefix,
                    PickerItem::action(
                        format!("{}: set value", spec.key),
                        format!("/settings set {} ", spec.key),
                    )
                    .detail(&detail)
                    .badge("INPUT")
                    .prefill(),
                ));
            }
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
    let (tabs, items) = grouped_setting_rows(rows, &lang);
    PickerSpec::new(crate::cli::i18n::tr(&lang, "view.settings.title"), items)
        .with_tabs(tabs)
        .localized(&lang)
}
pub(crate) fn contract_settings() -> Value {
    let config = read_user_writable_config();
    let communication = setting_spec(COMMUNICATION_CONTRACT_KEY)
        .map(|spec| effective_setting_value(&config, spec))
        .unwrap_or_else(|| json!(""));
    let functionality = setting_spec(FUNCTIONALITY_CONTRACT_KEY)
        .map(|spec| effective_setting_value(&config, spec))
        .unwrap_or_else(|| json!(""));
    json!({
        "communication": communication,
        "functionality": functionality,
        "path": user_config_path().display().to_string(),
        "taskContract": crate::agent::task_contract::snapshot(
            &super::ui_language(&crate::load_config(Path::new(".")))
        ),
    })
}

pub(crate) fn set_contract_settings(
    communication: &str,
    functionality: &str,
) -> Result<Value, String> {
    let communication_spec =
        setting_spec(COMMUNICATION_CONTRACT_KEY).expect("communication contract setting");
    let functionality_spec =
        setting_spec(FUNCTIONALITY_CONTRACT_KEY).expect("functionality contract setting");
    let communication = parse_setting_value(communication_spec, communication)?;
    let functionality = parse_setting_value(functionality_spec, functionality)?;
    let mut config = read_user_writable_config();
    config_set_value(
        &mut config,
        COMMUNICATION_CONTRACT_KEY,
        communication.clone(),
    )?;
    config_set_value(
        &mut config,
        FUNCTIONALITY_CONTRACT_KEY,
        functionality.clone(),
    )?;
    let path = write_user_config(&config)?;
    Ok(json!({
        "communication": communication,
        "functionality": functionality,
        "path": path.display().to_string(),
        "taskContract": crate::agent::task_contract::snapshot(
            &super::ui_language(&crate::load_config(Path::new(".")))
        ),
    }))
}

const COMMUNICATION_KEYS: [&str; 5] = [
    COMMUNICATION_MODE_KEY,
    COMMUNICATION_TOOL_CALLS_KEY,
    COMMUNICATION_TOOL_RESULTS_KEY,
    COMMUNICATION_REASONING_KEY,
    COMMUNICATION_CODE_KEY,
];

fn communication_settings_json(config: &Value, path: &Path) -> Value {
    let mut out = serde_json::Map::new();
    for key in COMMUNICATION_KEYS {
        let spec = setting_spec(key).expect("communication setting");
        let field = key.rsplit('.').next().expect("dotted key");
        out.insert(field.to_string(), effective_setting_value(config, spec));
    }
    out.insert(
        "effective".into(),
        DisplayPolicy::for_cwd(&std::env::current_dir().unwrap_or_default()).json(),
    );
    out.insert("path".into(), json!(path.display().to_string()));
    Value::Object(out)
}

/// The user-default communication settings plus the policy in force here,
/// which also folds in the project layer of the current directory.
pub(crate) fn communication_settings() -> Value {
    communication_settings_json(&read_user_writable_config(), &user_config_path())
}

/// Write all five user-default communication settings atomically. Each value
/// is validated against the setting schema before anything is written.
pub(crate) fn set_communication_settings(values: &[(&str, &str); 5]) -> Result<Value, String> {
    let mut config = read_user_writable_config();
    for (key, raw) in values {
        let spec = setting_spec(key).ok_or_else(|| format!("unknown config key: {key}"))?;
        config_set_value(&mut config, key, parse_setting_value(spec, raw)?)?;
    }
    let path = write_user_config(&config)?;
    Ok(communication_settings_json(&config, &path))
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
