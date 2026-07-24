//! Configuration structs plus layered config discovery, load and deep-merge.

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use crate::cli::auth::AuthProviderConfig;
use crate::{config_path, legacy_user_config_path, user_config_path};

#[path = "../../migrations/mod.rs"]
pub(crate) mod migrations;
pub(crate) mod schema;

pub(crate) const CONFIG_SCHEMA_VERSION: u32 = 3;

fn config_v0_to_v1(value: &mut Value) -> Result<(), String> {
    value
        .as_object_mut()
        .ok_or_else(|| "config root must be an object".to_string())?;
    Ok(())
}

fn config_v1_to_v2(value: &mut Value) -> Result<(), String> {
    let object = value
        .as_object_mut()
        .ok_or_else(|| "config root must be an object".to_string())?;
    if let Some(legacy) = object.remove("model_url") {
        object.entry("modelRouterUrl").or_insert(legacy);
    }
    Ok(())
}

fn config_v2_to_v3(value: &mut Value) -> Result<(), String> {
    let object = value
        .as_object_mut()
        .ok_or_else(|| "config root must be an object".to_string())?;
    object.entry("billing").or_insert_with(|| {
        json!({
            "autoPurchaseEnabled": false,
            "autoRenewEnabled": false,
            "preferredCurrency": null,
            "maxSingleMicrounits": 0,
            "maxPeriodMicrounits": 0
        })
    });
    Ok(())
}

static CONFIG_MIGRATION_STEPS: [migrations::MigrationStep; 3] = [
    migrations::MigrationStep {
        name: "version-envelope",
        from: 0,
        to: 1,
        apply: config_v0_to_v1,
    },
    migrations::MigrationStep {
        name: "canonical-model-router-key",
        from: 1,
        to: 2,
        apply: config_v1_to_v2,
    },
    migrations::MigrationStep {
        name: "safe-billing-preferences",
        from: 2,
        to: 3,
        apply: config_v2_to_v3,
    },
];

pub(crate) fn config_migration_plan() -> migrations::MigrationPlan {
    migrations::MigrationPlan {
        store: "config",
        from: 0,
        to: CONFIG_SCHEMA_VERSION,
        reversible: true,
        preflight: migrations::object_preflight,
        steps: &CONFIG_MIGRATION_STEPS,
        compatibility_window: migrations::CompatibilityWindow {
            oldest_readable: 0,
            newest_readable: 3,
            rollback_floor: 2,
        },
    }
}

pub(crate) fn migrate_config_file(path: &Path) -> Result<migrations::MigrationOutcome, String> {
    migrations::migrate_json(path, &config_migration_plan())
}

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
    #[serde(default)]
    pub(crate) billing: BillingPreferencesConfig,
    #[serde(default)]
    pub(crate) ui: UiConfig,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub(crate) struct BillingPreferencesConfig {
    #[serde(rename = "autoPurchaseEnabled", default)]
    pub(crate) auto_purchase_enabled: bool,
    #[serde(rename = "autoRenewEnabled", default)]
    pub(crate) auto_renew_enabled: bool,
    #[serde(rename = "preferredCurrency", default)]
    pub(crate) preferred_currency: Option<String>,
    #[serde(rename = "maxSingleMicrounits", default)]
    pub(crate) max_single_microunits: u64,
    #[serde(rename = "maxPeriodMicrounits", default)]
    pub(crate) max_period_microunits: u64,
}

impl Default for BillingPreferencesConfig {
    fn default() -> Self {
        Self {
            auto_purchase_enabled: false,
            auto_renew_enabled: false,
            preferred_currency: None,
            max_single_microunits: 0,
            max_period_microunits: 0,
        }
    }
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

/// Languages offered by wisent-app (src/locales) — the same set is pinnable here.
pub(crate) const UI_LANGUAGE_CODES: &[&str] = &[
    "am", "ar", "az", "be", "bg", "bn", "bs", "ca", "cs", "da", "de", "dv", "dz", "el", "en", "es",
    "et", "fa", "fi", "fo", "fr", "he", "hr", "hu", "hy", "id", "is", "it", "ja", "ka", "kk", "kl",
    "km", "ko", "ky", "lo", "lt", "lv", "mk", "mn", "ms", "my", "ne", "nl", "no", "pl", "ps", "pt",
    "ro", "ru", "si", "sk", "sl", "so", "sq", "sr", "sv", "tg", "th", "tk", "tr", "uk", "uz", "vi",
    "zh",
];

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(transparent)]
pub(crate) struct UiLanguage(String);

impl Default for UiLanguage {
    fn default() -> Self {
        Self("auto".into())
    }
}

impl UiLanguage {
    fn parse(value: &str) -> Option<Self> {
        let value = value.trim().to_ascii_lowercase();
        if value == "auto" || UI_LANGUAGE_CODES.contains(&value.as_str()) {
            Some(Self(value))
        } else {
            None
        }
    }
    pub(crate) fn code(&self) -> &str {
        &self.0
    }
    pub(crate) fn is_auto(&self) -> bool {
        self.0 == "auto"
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub(crate) struct UiConfig {
    #[serde(default)]
    pub(crate) language: UiLanguage,
    #[serde(default = "default_ui_theme")]
    pub(crate) theme: String,
}

fn default_ui_theme() -> String {
    "auto".into()
}

/// The configured `ui.theme` value ("auto" when unset).
pub(crate) fn ui_theme() -> String {
    crate::cli::config::merged_config_value(&std::env::current_dir().unwrap_or_default())
        .get("ui")
        .and_then(|ui| ui.get("theme"))
        .and_then(serde_json::Value::as_str)
        .unwrap_or("auto")
        .to_string()
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
    if path.exists() {
        migrate_config_file(&path)?;
    }
    let mut versioned = value.clone();
    let object = versioned
        .as_object_mut()
        .ok_or_else(|| "config root must be an object".to_string())?;
    object.insert("schemaVersion".into(), json!(CONFIG_SCHEMA_VERSION));
    migrations::write_json_atomic(&path, &versioned)?;
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
