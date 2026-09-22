//! What the gateway says about the models it can route to, and how a model is
//! resolved out of that.
//!
//! Split out of `control_plane/services/brama.rs`, which had grown past the
//! module line cap.

use super::{BramaError, API_VERSION};
use serde::{Deserialize, Serialize};

/// Brama's own readiness verdict, as `/readyz` reports it.
#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct BramaReadiness {
    pub status: u16,
    pub ready: bool,
    pub degraded: bool,
    pub operator_action_required: bool,
    pub reason: String,
    pub providers_without_credential: Vec<String>,
}
#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct ModelPrice {
    #[serde(default)]
    pub input: f64,
    #[serde(default)]
    pub output: f64,
    #[serde(default)]
    pub cache_read: f64,
    #[serde(default)]
    pub cache_write: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct ModelPerf {
    #[serde(default)]
    pub count: u64,
    #[serde(default)]
    pub latency_ms: f64,
    #[serde(default)]
    pub tps: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct ModelEntry {
    pub id: String,
    #[serde(default = "default_true")]
    pub available: bool,
    #[serde(default)]
    pub context_window: u64,
    #[serde(default)]
    pub max_output_tokens: u64,
    #[serde(default)]
    pub input_modalities: Vec<String>,
    #[serde(default)]
    pub output_modalities: Vec<String>,
    #[serde(default)]
    pub tools: bool,
    #[serde(default)]
    pub reasoning: bool,
    #[serde(default, alias = "cost", alias = "pricing")]
    pub price: ModelPrice,
    #[serde(default)]
    pub fallback: Vec<String>,
    #[serde(default)]
    pub promotion: Vec<String>,
    #[serde(default)]
    pub unavailable_reason: Option<String>,
    #[serde(default)]
    pub perf: Option<ModelPerf>,
}

fn default_true() -> bool {
    true
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct ModelCatalog {
    #[serde(default, alias = "revision")]
    pub catalog_revision: String,
    #[serde(default = "api_version")]
    pub version: String,
    #[serde(default)]
    pub models: Vec<ModelEntry>,
    #[serde(default)]
    pub degraded: bool,
}
fn api_version() -> String {
    API_VERSION.into()
}

impl ModelCatalog {
    pub fn resolve(&self, id: &str) -> Result<&ModelEntry, BramaError> {
        let model = self
            .models
            .iter()
            .find(|model| model.id == id)
            .ok_or_else(|| BramaError::UnknownModel(id.to_string()))?;
        if !model.available {
            return Err(BramaError::UnavailableModel {
                model: id.to_string(),
                reason: model
                    .unavailable_reason
                    .clone()
                    .unwrap_or_else(|| "catalog marks route unavailable".into()),
            });
        }
        Ok(model)
    }

    pub fn price(&self, id: &str) -> Option<&ModelPrice> {
        self.models
            .iter()
            .find(|model| model.id == id && model.available)
            .map(|model| &model.price)
    }

    /// Resolve a bare (provider-less) model id to a catalog entry whose id
    /// ends with `/<model>`. Returns the entry on a unique match, `None` when
    /// nothing matches, or an error naming every matching route id when the
    /// bare id is ambiguous.
    pub fn resolve_bare(&self, model: &str) -> Result<Option<&ModelEntry>, String> {
        let suffix = format!("/{model}");
        let matches = self
            .models
            .iter()
            .filter(|entry| entry.id.ends_with(suffix.as_str()))
            .collect::<Vec<_>>();
        match matches.len() {
            0 => Ok(None),
            1 => Ok(matches.into_iter().next()),
            _ => Err(format!(
                "model `{model}` is ambiguous; it matches multiple Brama routes: {}; use the full route id",
                matches
                    .iter()
                    .map(|entry| entry.id.as_str())
                    .collect::<Vec<_>>()
                    .join(", ")
            )),
        }
    }
}

pub(super) fn validate_catalog(catalog: &ModelCatalog) -> Result<(), BramaError> {
    if catalog.version != API_VERSION {
        return Err(BramaError::InvalidCatalog(format!(
            "unsupported version `{}`",
            catalog.version
        )));
    }
    let mut ids = std::collections::HashSet::with_capacity(catalog.models.len());
    for model in &catalog.models {
        if model.id.trim().is_empty() {
            return Err(BramaError::InvalidCatalog("model id is empty".into()));
        }
        if !ids.insert(&model.id) {
            return Err(BramaError::InvalidCatalog(format!(
                "duplicate model `{}`",
                model.id
            )));
        }
        if !model.price.input.is_finite()
            || !model.price.output.is_finite()
            || model.price.input < 0.0
            || model.price.output < 0.0
        {
            return Err(BramaError::InvalidCatalog(format!(
                "invalid price for `{}`",
                model.id
            )));
        }
    }
    Ok(())
}
