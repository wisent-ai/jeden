//! The operator's two contracts and the communication settings, read and
//! written as a set.
//!
//! Split out of `cli/config/schema.rs`, which had grown past the module line
//! cap.

use super::super::{read_user_writable_config_strict, write_user_config};
use super::values::{effective_setting_value, parse_setting_value, setting_spec};
use super::{
    COMMUNICATION_CODE_KEY, COMMUNICATION_CONTRACT_KEY, COMMUNICATION_MODE_KEY,
    COMMUNICATION_REASONING_KEY, COMMUNICATION_TOOL_CALLS_KEY, COMMUNICATION_TOOL_RESULTS_KEY,
    FUNCTIONALITY_CONTRACT_KEY,
};
use crate::cli::config::communication::DisplayPolicy;
use crate::cli::config::config_set_value;
use crate::user_config_path;
use serde_json::{json, Value};
use std::path::Path;

/// The two operator contracts plus what Jeden actually uses for
/// communication: `communicationSource` says whether the text in force is
/// Jeden's default, the operator's own, or nothing, and
/// `communicationDefault` is the default text for the current language. A
/// user configuration that exists and cannot be read answers `refusal` with
/// its path and cause, never the defaults.
pub(crate) fn contract_settings() -> Value {
    match read_contract_settings() {
        Ok(settings) => settings,
        Err(cause) => refused(cause),
    }
}

/// What a getter answers when the user configuration cannot be read.
fn refused(cause: String) -> Value {
    json!({ "refusal": cause, "path": user_config_path().display().to_string() })
}

/// A setting this build declares; one missing from the compiled schema is a
/// defect in this file's key, refused naming the key rather than panicking
/// the process that asked.
fn declared(key: &str) -> Result<&'static super::SettingSpec, String> {
    setting_spec(key).ok_or_else(|| {
        format!("{key} is not in this build's compiled settings schema, so it cannot be read or written")
    })
}

fn read_contract_settings() -> Result<Value, String> {
    let config = read_user_writable_config_strict()?;
    let communication = effective_setting_value(&config, declared(COMMUNICATION_CONTRACT_KEY)?);
    let functionality = effective_setting_value(&config, declared(FUNCTIONALITY_CONTRACT_KEY)?);
    contract_settings_json(communication, functionality, &user_config_path())
}

fn contract_settings_json(
    communication: Value,
    functionality: Value,
    path: &Path,
) -> Result<Value, String> {
    let language = super::ui_language(&crate::load_config(Path::new(".")));
    let stated = communication.as_str().ok_or_else(|| {
        format!("{COMMUNICATION_CONTRACT_KEY} in {} is not text: {communication}", path.display())
    })?;
    let (source, _) = crate::agent::communication_contract::resolve(stated, &language);
    Ok(json!({
        "communication": communication,
        "functionality": functionality,
        "communicationSource": source.as_str(),
        "communicationDefault": crate::agent::communication_contract::default_text(&language),
        "path": path.display().to_string(),
        "taskContract": crate::agent::task_contract::snapshot(&language),
    }))
}

pub(crate) fn set_contract_settings(
    communication: &str,
    functionality: &str,
) -> Result<Value, String> {
    let communication_spec = declared(COMMUNICATION_CONTRACT_KEY)?;
    let functionality_spec = declared(FUNCTIONALITY_CONTRACT_KEY)?;
    let communication = parse_setting_value(communication_spec, communication)?;
    let functionality = parse_setting_value(functionality_spec, functionality)?;
    let mut config = read_user_writable_config_strict()?;
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
    contract_settings_json(communication, functionality, &path)
}

/// The user-default communication settings, written and answered as one set.
const COMMUNICATION_KEYS: &[&str] = &[
    COMMUNICATION_MODE_KEY,
    COMMUNICATION_TOOL_CALLS_KEY,
    COMMUNICATION_TOOL_RESULTS_KEY,
    COMMUNICATION_REASONING_KEY,
    COMMUNICATION_CODE_KEY,
];

fn communication_settings_json(config: &Value, path: &Path) -> Result<Value, String> {
    let mut out = serde_json::Map::new();
    for key in COMMUNICATION_KEYS {
        let Some((_section, field)) = key.rsplit_once('.') else {
            return Err(format!(
                "the communication setting {key} names no section, so its field cannot be answered"
            ));
        };
        out.insert(field.to_string(), effective_setting_value(config, declared(key)?));
    }
    // The policy in force where this process runs, project layer included.
    let here = std::env::current_dir().map_err(|error| {
        format!("the communication policy in force here cannot be read: this process's working directory is unreadable ({error})")
    })?;
    out.insert("effective".into(), DisplayPolicy::for_cwd(&here).json());
    out.insert("path".into(), json!(path.display().to_string()));
    Ok(Value::Object(out))
}

/// The user-default communication settings plus the policy in force here,
/// which also folds in the project layer of the current directory; refused
/// with the path and cause when the user configuration cannot be read.
pub(crate) fn communication_settings() -> Value {
    match read_user_writable_config_strict()
        .and_then(|config| communication_settings_json(&config, &user_config_path()))
    {
        Ok(settings) => settings,
        Err(cause) => refused(cause),
    }
}

/// Write every user-default communication setting atomically: `values` names
/// each of them once, and each value is validated against the setting schema
/// before anything is written.
pub(crate) fn set_communication_settings(values: &[(&str, &str)]) -> Result<Value, String> {
    for key in COMMUNICATION_KEYS {
        let stated = values.iter().filter(|(name, _)| name == key).count();
        if stated != usize::from(crate::tally::STEP) {
            return Err(format!(
                "the communication settings are written as a set: {key} is stated {stated} times, not once"
            ));
        }
    }
    let mut config = read_user_writable_config_strict()?;
    for (key, raw) in values {
        let spec = setting_spec(key).ok_or_else(|| format!("unknown config key: {key}"))?;
        config_set_value(&mut config, key, parse_setting_value(spec, raw)?)?;
    }
    let path = write_user_config(&config)?;
    communication_settings_json(&config, &path)
}
