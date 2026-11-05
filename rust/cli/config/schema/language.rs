//! `ui.language` as the Settings screens read and write it: the value saved,
//! and every language a person can pin with its English and native name, so
//! a screen offers the fleet's list (wisent-texts) instead of keeping one.

use super::super::{read_user_writable_config_strict, write_user_config};
use super::values::{effective_setting_value, parse_setting_value, setting_spec};
use super::UI_LANGUAGE_AUTO;
use crate::cli::config::config_set_value;
use crate::user_config_path;
use serde_json::{json, Value};

const KEY: &str = "ui.language";

fn spec() -> Result<&'static super::SettingSpec, String> {
    setting_spec(KEY).ok_or_else(|| {
        format!("{KEY} is not in this build's compiled settings schema, so it cannot be read or written")
    })
}

fn answer(config: &Value) -> Result<Value, String> {
    let languages: Vec<Value> = wisent_texts::languages()
        .iter()
        .map(|language| json!({"code": language.code, "english": language.english, "native": language.native}))
        .collect();
    Ok(json!({
        "language": effective_setting_value(config, spec()?),
        "auto": UI_LANGUAGE_AUTO,
        "languages": languages,
        "path": user_config_path().display().to_string(),
    }))
}

/// The saved `ui.language` and the languages it may take; a user
/// configuration that exists and cannot be read answers `refusal` with its
/// path and cause, never the default.
pub(crate) fn language_settings() -> Value {
    match read_user_writable_config_strict().and_then(|config| answer(&config)) {
        Ok(settings) => settings,
        Err(cause) => json!({ "refusal": cause, "path": user_config_path().display().to_string() }),
    }
}

/// Save `ui.language`: `auto` or one code of the fleet list, validated by the
/// setting schema before anything is written.
pub(crate) fn set_language_settings(language: &str) -> Result<Value, String> {
    let value = parse_setting_value(spec()?, language)
        .map_err(|cause| format!("{language:?} is not a language Jeden offers: {cause}"))?;
    let mut config = read_user_writable_config_strict()?;
    config_set_value(&mut config, KEY, value)?;
    write_user_config(&config)?;
    answer(&config)
}
