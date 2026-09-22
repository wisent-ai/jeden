//! Configuration: what a document may contain, and how the layers that apply
//! here are found, merged and written.

use serde_json::Value;
use std::collections::BTreeMap;
use std::path::Path;

pub(crate) mod communication;
mod document;
#[path = "../../migrations/mod.rs"]
pub(crate) mod migrations;
pub(crate) mod schema;
mod shapes;

pub(crate) use document::{config_layer_paths, config_remove_value, config_set_value, config_value_at, merged_config_value, parse_config_literal, read_config_typed, read_user_writable_config, read_user_writable_config_strict, write_user_config};
pub(crate) use shapes::*;

pub(crate) const CONFIG_SCHEMA_VERSION: u32 = 4;

pub(crate) fn load_config(cwd: &Path) -> Config {
    let merged: Config = serde_json::from_value(merged_config_value(cwd)).unwrap_or_default();
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
/// which already layers project over user. Invalid env values fall through.
pub(crate) fn ui_language(config: &Config) -> UiLanguage {
    std::env::var("JEDEN_LANGUAGE")
        .ok()
        .and_then(|value| UiLanguage::parse(&value))
        .unwrap_or_else(|| config.ui.language.clone())
}
