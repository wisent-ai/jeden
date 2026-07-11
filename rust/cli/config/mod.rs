//! Configuration structs plus layered config discovery, load and deep-merge.

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use crate::cli::auth::AuthProviderConfig;
use crate::{config_path, legacy_user_config_path, user_config_path};

pub(crate) mod schema;

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub(crate) struct Config {
    #[serde(rename = "model")]
    pub(crate) model: Option<String>,
    #[serde(rename = "modelRouterUrl")]
    pub(crate) model_router_url: Option<String>,
    #[serde(rename = "agentId")]
    pub(crate) agent_id: Option<String>,
    #[serde(rename = "authProviders")]
    pub(crate) auth_providers: Option<BTreeMap<String, AuthProviderConfig>>,
    #[serde(default)]
    pub(crate) models: Vec<ModelConfig>,
    #[serde(rename = "modelOverrides", default)]
    pub(crate) model_overrides: BTreeMap<String, ModelOverrideConfig>,
    #[serde(default)]
    pub(crate) context: ContextConfig,
    #[serde(default)]
    pub(crate) rules: RulesConfig,
    #[serde(default)]
    pub(crate) secrets: SecretsConfig,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct ContextConfig {
    #[serde(rename = "maxBytes", default = "default_context_max_bytes")]
    pub(crate) max_bytes: usize,
    #[serde(rename = "maxTokens", default = "default_context_max_tokens")]
    pub(crate) max_tokens: usize,
}

impl Default for ContextConfig {
    fn default() -> Self {
        Self {
            max_bytes: default_context_max_bytes(),
            max_tokens: default_context_max_tokens(),
        }
    }
}

fn default_context_max_bytes() -> usize {
    131_072
}

fn default_context_max_tokens() -> usize {
    32_768
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub(crate) struct RulesConfig {
    #[serde(rename = "alwaysApply", default)]
    pub(crate) always_apply: Vec<AlwaysApplyRuleConfig>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub(crate) struct AlwaysApplyRuleConfig {
    pub(crate) id: String,
    pub(crate) content: Option<String>,
    pub(crate) source: Option<PathBuf>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, Default)]
#[serde(rename_all = "kebab-case")]
pub(crate) enum SecretMode {
    #[default]
    Redact,
    Obfuscate,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct SecretsConfig {
    #[serde(default)]
    pub(crate) mode: SecretMode,
    #[serde(default = "default_secret_replacement")]
    pub(crate) replacement: String,
    #[serde(rename = "minLength", default = "default_secret_min_length")]
    pub(crate) min_length: usize,
    #[serde(default)]
    pub(crate) values: Vec<String>,
    #[serde(default)]
    pub(crate) environment: Vec<String>,
    #[serde(default)]
    pub(crate) files: Vec<PathBuf>,
    #[serde(rename = "discoverEnvironment", default = "default_true")]
    pub(crate) discover_environment: bool,
}

impl Default for SecretsConfig {
    fn default() -> Self {
        Self {
            mode: SecretMode::Redact,
            replacement: default_secret_replacement(),
            min_length: default_secret_min_length(),
            values: Vec::new(),
            environment: Vec::new(),
            files: Vec::new(),
            discover_environment: true,
        }
    }
}

fn default_secret_replacement() -> String {
    "[REDACTED]".to_string()
}

fn default_secret_min_length() -> usize {
    8
}

fn default_true() -> bool {
    true
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub(crate) struct ModelConfig {
    pub(crate) id: String,
    pub(crate) cost: Option<ModelCostConfig>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub(crate) struct ModelOverrideConfig {
    pub(crate) cost: Option<ModelCostConfig>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub(crate) struct ModelCostConfig {
    pub(crate) input: Option<f64>,
    pub(crate) output: Option<f64>,
    #[serde(rename = "cacheRead")]
    pub(crate) cache_read: Option<f64>,
    #[serde(rename = "cacheWrite")]
    pub(crate) cache_write: Option<f64>,
}

pub(crate) fn read_config_value(path: &Path) -> Value {
    let Some(text) = fs::read_to_string(path).ok() else {
        return json!({});
    };
    let parsed = match path
        .extension()
        .and_then(|ext| ext.to_str())
        .unwrap_or("")
        .to_ascii_lowercase()
        .as_str()
    {
        "yml" | "yaml" => serde_yaml::from_str::<Value>(&text).ok(),
        _ => serde_json::from_str::<Value>(&text).ok(),
    };
    parsed.filter(Value::is_object).unwrap_or_else(|| json!({}))
}

fn read_config_typed<T: for<'a> Deserialize<'a> + Default>(path: &Path) -> T {
    serde_json::from_value(read_config_value(path)).unwrap_or_default()
}

fn global_config_layer_paths() -> Vec<PathBuf> {
    vec![legacy_user_config_path(), user_config_path()]
}

fn project_config_layer_paths(cwd: &Path) -> Vec<PathBuf> {
    vec![config_path(cwd)]
}

fn config_layer_paths(cwd: &Path) -> Vec<PathBuf> {
    let mut paths = global_config_layer_paths();
    paths.extend(project_config_layer_paths(cwd));
    paths
}

fn deep_merge_value(base: &mut Value, overlay: Value) {
    match (base, overlay) {
        (Value::Object(base), Value::Object(overlay)) => {
            for (key, value) in overlay {
                if let Some(existing) = base.get_mut(&key) {
                    deep_merge_value(existing, value);
                } else {
                    base.insert(key, value);
                }
            }
        }
        (base, overlay) => *base = overlay,
    }
}

pub(crate) fn merged_config_value(cwd: &Path) -> Value {
    let mut merged = json!({});
    for path in config_layer_paths(cwd) {
        deep_merge_value(&mut merged, read_config_value(&path));
    }
    merged
}

pub(crate) fn config_value_at<'a>(value: &'a Value, key: &str) -> Option<&'a Value> {
    let mut current = value;
    for part in key.split('.') {
        if part.is_empty() {
            return None;
        }
        current = current.get(part)?;
    }
    Some(current)
}

pub(crate) fn config_set_value(value: &mut Value, key: &str, next: Value) -> Result<(), String> {
    if !value.is_object() {
        *value = json!({});
    }
    let parts = key
        .split('.')
        .filter(|part| !part.is_empty())
        .collect::<Vec<_>>();
    let Some((last, prefix)) = parts.split_last() else {
        return Err("config key is required".into());
    };
    let mut current = value;
    for part in prefix {
        if !current.get(*part).map(Value::is_object).unwrap_or(false) {
            current
                .as_object_mut()
                .expect("object")
                .insert((*part).to_string(), json!({}));
        }
        current = current.get_mut(*part).expect("inserted object");
    }
    current
        .as_object_mut()
        .expect("object")
        .insert((*last).to_string(), next);
    Ok(())
}

pub(crate) fn parse_config_literal(raw: &str) -> Value {
    let trimmed = raw.trim();
    if trimmed.eq_ignore_ascii_case("true") {
        return json!(true);
    }
    if trimmed.eq_ignore_ascii_case("false") {
        return json!(false);
    }
    if let Ok(number) = trimmed.parse::<f64>() {
        if number.is_finite() {
            return json!(number);
        }
    }
    serde_json::from_str::<Value>(trimmed).unwrap_or_else(|_| json!(trimmed))
}

pub(crate) fn read_user_writable_config() -> Value {
    let current = read_config_value(&user_config_path());
    if current
        .as_object()
        .map(|map| !map.is_empty())
        .unwrap_or(false)
    {
        current
    } else {
        read_config_value(&legacy_user_config_path())
    }
}

pub(crate) fn write_user_config(value: &Value) -> Result<PathBuf, String> {
    let path = user_config_path();
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|error| error.to_string())?;
    }
    fs::write(
        &path,
        serde_yaml::to_string(value).map_err(|error| error.to_string())?,
    )
    .map_err(|error| error.to_string())?;
    Ok(path)
}

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
        model_router_url: merged.model_router_url,
        agent_id: merged.agent_id,
        auth_providers: merged.auth_providers,
        models: model_catalog.into_values().collect(),
        model_overrides,
        context: merged.context,
        rules: merged.rules,
        secrets: merged.secrets,
    }
}
