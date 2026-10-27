//! Configuration: what a document may contain, and how the layers that apply
//! here are found, merged and written.

use std::collections::BTreeMap;
use std::path::Path;

pub(crate) mod communication;
mod document;
pub(crate) mod env_files;
#[path = "../../migrations/mod.rs"]
pub(crate) mod migrations;
pub(crate) mod schema;
mod shapes;

pub(crate) use document::{
    config_layer_paths, config_remove_value, config_set_value, config_texts, config_value_at,
    merged_config_value, read_config_layer, read_config_typed,
    read_config_value, read_user_writable_config_strict, user_config_value, write_user_config,
};
pub(crate) use shapes::*;

/// Every configuration layer that applies here, read the way a run reads it.
/// `jeden` refuses to start while one cannot be read, does not parse, or holds
/// values the settings cannot take, naming the file and the cause; read as
/// empty, such a layer dropped every setting in it without a word.
pub(crate) fn check_config(cwd: &Path) -> Result<(), String> {
    for path in config_layer_paths(cwd) {
        let Some(layer) = read_config_layer(&path)? else {
            continue;
        };
        serde_json::from_value::<Config>(layer).map_err(|error| {
            format!(
                "configuration {} does not hold valid settings: {error}",
                path.display()
            )
        })?;
    }
    Ok(())
}

pub(crate) fn load_config(cwd: &Path) -> Config {
    let merged: Config = match serde_json::from_value(merged_config_value(cwd)) {
        Ok(merged) => merged,
        Err(error) => panic!("the merged configuration does not hold valid settings: {error}"),
    };
    let mut model_catalog = BTreeMap::new();
    let mut model_overrides = BTreeMap::new();
    for path in config_layer_paths(cwd) {
        let layer: Config = read_config_typed(&path);
        for model in layer.models {
            model_catalog.insert(model.id.clone(), model);
        }
        model_overrides.extend(layer.model_overrides);
    }
    Config {
        model: merged.model,
        agent_id: merged.agent_id,
        auth_providers: merged.auth_providers,
        models: model_catalog.into_values().collect(),
        model_overrides,
        context: merged.context,
        rules: merged.rules,
        contracts: merged.contracts,
        communication: merged.communication,
        secrets: merged.secrets,
        billing: merged.billing,
        ui: merged.ui,
    }
}

/// Resolve the conversation language: JEDEN_LANGUAGE wins over merged config,
/// which already layers project over user. A value Jeden cannot use is said on
/// stderr, naming it, and the configured language is used.
pub(crate) fn ui_language(config: &Config) -> UiLanguage {
    match std::env::var("JEDEN_LANGUAGE") {
        Err(std::env::VarError::NotPresent) => config.ui.language.clone(),
        Err(std::env::VarError::NotUnicode(_)) => {
            eprintln!("jeden: JEDEN_LANGUAGE is not Unicode; using the configured ui.language");
            config.ui.language.clone()
        }
        Ok(value) => match UiLanguage::parse(&value) {
            Some(language) => language,
            None => {
                eprintln!(
                    "jeden: JEDEN_LANGUAGE={value:?} is not a language Jeden speaks; using the configured ui.language"
                );
                config.ui.language.clone()
            }
        },
    }
}
