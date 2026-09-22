//! What a configuration document is allowed to contain, declared as types
//! with their defaults beside them.
//!
//! Split out of `cli/config/mod.rs`, which had grown past the module line cap.

use super::{communication, schema};
use crate::cli::auth::AuthProviderConfig;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::PathBuf;

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub(crate) struct Config {
    #[serde(rename = "model")]
    pub(crate) model: Option<String>,
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
    pub(crate) contracts: ContractsConfig,
    #[serde(default)]
    pub(crate) communication: communication::CommunicationConfig,
    #[serde(default)]
    pub(crate) secrets: SecretsConfig,
    #[serde(default)]
    pub(crate) billing: BillingPreferencesConfig,
    #[serde(default)]
    pub(crate) ui: UiConfig,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
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

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct ContextConfig {
    #[serde(rename = "maxBytes", default = "default_context_max_bytes")]
    pub(crate) max_bytes: usize,
    #[serde(rename = "maxTokens", default = "default_context_max_tokens")]
    pub(crate) max_tokens: usize,
    #[serde(default)]
    pub(crate) advisor: AdvisorConfig,
}

impl Default for ContextConfig {
    fn default() -> Self {
        Self {
            max_bytes: default_context_max_bytes(),
            max_tokens: default_context_max_tokens(),
            advisor: AdvisorConfig::default(),
        }
    }
}

/// What the context advisor reads and how much of it reaches a prompt. The
/// source list, the roots and the endpoints are declarations rather than
/// code so a machine's corpus is the operator's, not the binary's.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct AdvisorConfig {
    #[serde(default = "default_true")]
    pub(crate) enabled: bool,
    #[serde(default = "default_advisor_limit")]
    pub(crate) limit: usize,
    #[serde(rename = "maxChars", default = "default_advisor_max_chars")]
    pub(crate) max_chars: usize,
    #[serde(default = "default_advisor_sources")]
    pub(crate) sources: String,
    /// Colon-separated `path` or `path@depth` entries. Empty means the
    /// project and the operator's own Jeden instruction directory.
    #[serde(default)]
    pub(crate) roots: String,
    #[serde(rename = "fileExtensions", default = "default_advisor_file_extensions")]
    pub(crate) file_extensions: String,
    #[serde(rename = "groundTruthUrl", default)]
    pub(crate) ground_truth_url: String,
    #[serde(rename = "transcriptLakeBin", default)]
    pub(crate) transcript_lake_bin: String,
}

impl Default for AdvisorConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            limit: default_advisor_limit(),
            max_chars: default_advisor_max_chars(),
            sources: default_advisor_sources(),
            roots: String::new(),
            file_extensions: default_advisor_file_extensions(),
            ground_truth_url: String::new(),
            transcript_lake_bin: String::new(),
        }
    }
}

fn default_context_max_bytes() -> usize {
    131_072
}

fn default_context_max_tokens() -> usize {
    32_768
}

fn default_advisor_limit() -> usize {
    crate::context::advisor::DEFAULT_LIMIT
}

fn default_advisor_max_chars() -> usize {
    crate::context::advisor::DEFAULT_MAX_CHARS
}

fn default_advisor_sources() -> String {
    crate::context::advisor::DEFAULT_SOURCES.to_string()
}

fn default_advisor_file_extensions() -> String {
    crate::context::advisor::DEFAULT_FILE_EXTENSIONS.to_string()
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub(crate) struct RulesConfig {
    #[serde(rename = "alwaysApply", default)]
    pub(crate) always_apply: Vec<AlwaysApplyRuleConfig>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
pub(crate) struct ContractsConfig {
    #[serde(default)]
    pub(crate) communication: String,
    #[serde(default)]
    pub(crate) functionality: String,
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

/// Languages offered by wisent-app (src/locales) — the same set is pinnable
/// here. Declared in `ui-languages.json` beside this module, so the parser
/// and the settings schema read one list and neither can drift from it.
pub(crate) fn ui_language_codes() -> &'static [String] {
    static DECLARED: std::sync::LazyLock<Vec<String>> = std::sync::LazyLock::new(|| {
        let document: serde_json::Value = serde_json::from_str(include_str!("ui-languages.json"))
            .expect("ui-languages.json beside this module is valid JSON");
        document["words"]
            .as_array()
            .expect("ui-languages.json declares a words array")
            .iter()
            .filter_map(|code| code.as_str().map(str::to_owned))
            .collect()
    });
    &DECLARED
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(transparent)]
pub(crate) struct UiLanguage(String);

impl Default for UiLanguage {
    fn default() -> Self {
        Self("auto".into())
    }
}

impl UiLanguage {
    pub(super) fn parse(value: &str) -> Option<Self> {
        let value = value.trim().to_ascii_lowercase();
        if value == schema::UI_LANGUAGE_AUTO
            || ui_language_codes().iter().any(|code| *code == value)
        {
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
