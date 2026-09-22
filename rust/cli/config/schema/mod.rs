//! Settings schema, per-key parse and metadata, list rendering, and the config subcommand.

use serde_json::json;

use super::{config_remove_value, config_set_value, merged_config_value, read_user_writable_config, write_user_config};

use crate::user_config_path;
use crate::Args;

mod contracts;
mod table;
mod values;
mod views;

pub(crate) use contracts::{
    communication_settings, contract_settings, set_communication_settings, set_contract_settings,
};
pub(crate) use views::settings_picker;
use values::{effective_setting_value, parse_setting_value, setting_spec};
use views::{config_list_json, config_list_text};
use crate::cli::config::schema::table::SETTINGS_SCHEMA;
use crate::cli::config::schema::values::setting_default;
use crate::cli::config::schema::values::setting_metadata;
use crate::cli::config::shapes::ui_language_codes;
use crate::cli::config::ui_language;

/// The value that follows the user's own messages instead of pinning one
/// language.
pub(crate) const UI_LANGUAGE_AUTO: &str = "auto";

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

/// The settings this CLI has, and what each one accepts.
///
/// Built at first use rather than as a `const`, because one entry's
/// choices come from a declaration read at run time — the pinnable
/// languages — and a const cannot read a file.
pub(crate) fn settings_schema() -> &'static [SettingSpec] {
    &SETTINGS_SCHEMA
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
        "unset" => {
            let key = rest.first().ok_or("config unset requires a key")?;
            let spec = setting_spec(key).ok_or_else(|| format!("unknown config key: {key}"))?;
            let mut config = read_user_writable_config();
            let removed = config_remove_value(&mut config, key)?;
            let path = write_user_config(&config)?;
            let default_value = setting_default(spec);
            Ok(if args.json {
                serde_json::to_string_pretty(&json!({"key": key, "removed": removed, "value": default_value, "type": spec.typ, "description": spec.description, "path": path})).map_err(|error| error.to_string())? + "\n"
            } else if removed {
                format!(
                    "Removed {key} from {}; it reads {} again\n",
                    path.display(),
                    default_value
                )
            } else {
                format!("{key} was not written in {}\n", path.display())
            })
        }
        _ => Err(
            "Usage: jeden config [list|path|get <key>|set <key> <value>|reset <key>|unset <key>] \
             [--json]"
                .into(),
        ),
    }
}
