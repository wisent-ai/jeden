//! The operator's two contracts and the communication settings, read and
//! written as a set.
//!
//! Split out of `cli/config/schema.rs`, which had grown past the module line
//! cap.

use super::super::{read_user_writable_config, write_user_config};
use super::values::{effective_setting_value, parse_setting_value, setting_metadata, setting_spec};
use super::{
    COMMUNICATION_CODE_KEY, COMMUNICATION_CONTRACT_KEY, COMMUNICATION_MODE_KEY,
    COMMUNICATION_REASONING_KEY, COMMUNICATION_TOOL_CALLS_KEY, COMMUNICATION_TOOL_RESULTS_KEY,
    FUNCTIONALITY_CONTRACT_KEY,
};
use crate::user_config_path;
use serde_json::{json, Value};
use std::path::Path;

/// The two operator contracts plus what Jeden actually uses for
/// communication: `communicationSource` says whether the text in force is
/// Jeden's default, the operator's own, or nothing, and
/// `communicationDefault` is the default text for the current language.
pub(crate) fn contract_settings() -> Value {
    let config = read_user_writable_config();
    let communication = setting_spec(COMMUNICATION_CONTRACT_KEY)
        .map(|spec| effective_setting_value(&config, spec))
        .unwrap_or_else(|| json!(""));
    let functionality = setting_spec(FUNCTIONALITY_CONTRACT_KEY)
        .map(|spec| effective_setting_value(&config, spec))
        .unwrap_or_else(|| json!(""));
    contract_settings_json(communication, functionality, &user_config_path())
}

fn contract_settings_json(communication: Value, functionality: Value, path: &Path) -> Value {
    let language = super::ui_language(&crate::load_config(Path::new(".")));
    let (source, _) = crate::agent::communication_contract::resolve(
        communication.as_str().unwrap_or_default(),
        &language,
    );
    json!({
        "communication": communication,
        "functionality": functionality,
        "communicationSource": source.as_str(),
        "communicationDefault": crate::agent::communication_contract::default_text(&language),
        "path": path.display().to_string(),
        "taskContract": crate::agent::task_contract::snapshot(&language),
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
    Ok(contract_settings_json(communication, functionality, &path))
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
