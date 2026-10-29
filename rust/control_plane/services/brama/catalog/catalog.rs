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
/// A route's price per million tokens, side by side, as Brama's catalogue
/// states it: each side a number, or `null`/absent when the registry's
/// source states none. An unstated side is unknown, never free.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct ModelPrice {
    pub input: Option<f64>,
    pub output: Option<f64>,
    pub cache_read: Option<f64>,
    pub cache_write: Option<f64>,
}

impl ModelPrice {
    /// The tokens one price is the cost of: Brama passes on models.dev's
    /// prices, each in USD per million tokens
    /// (https://github.com/anomalyco/models.dev/blob/dev/README.md).
    pub const TOKENS_PER_PRICE: u32 = 1_000_000;

    /// Every side with its wire name, for a reader that must name the one
    /// it found unstated.
    pub fn sides(&self) -> Vec<(&'static str, Option<f64>)> {
        vec![
            ("input", self.input),
            ("output", self.output),
            ("cacheRead", self.cache_read),
            ("cacheWrite", self.cache_write),
        ]
    }
}

/// Brama's measured performance of a route; it always sends every field.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct ModelPerf {
    pub count: u64,
    pub latency_ms: f64,
    pub tps: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct ModelEntry {
    pub id: String,
    #[serde(default = "default_true")]
    pub available: bool,
    /// Brama answers `null` for a route whose registry declares no ceiling;
    /// that is "not declared", which a caller has to treat as such.
    #[serde(default)]
    pub context_window: Option<u64>,
    #[serde(default)]
    pub max_output_tokens: Option<u64>,
    #[serde(default)]
    pub input_modalities: Vec<String>,
    #[serde(default)]
    pub output_modalities: Vec<String>,
    /// Brama answers `null` for tools, reasoning and price of a route its
    /// registry holds no metadata for: unknown, never "no tools" or "free".
    #[serde(default)]
    pub tools: Option<bool>,
    #[serde(default)]
    pub reasoning: Option<bool>,
    #[serde(default, alias = "cost", alias = "pricing")]
    pub price: Option<ModelPrice>,
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
    #[serde(default)]
    pub version: String,
    /// Brama answers `models`; an OpenAI-compatible provider named by
    /// `JEDEN_MODEL_ENDPOINT` answers `data`, whose entries carry the same `id`.
    #[serde(default, alias = "data")]
    pub models: Vec<ModelEntry>,
    #[serde(default)]
    pub degraded: bool,
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
                reason: match &model.unavailable_reason {
                    Some(reason) => reason.clone(),
                    None => "the catalog marks the route unavailable and states no reason".into(),
                },
            });
        }
        Ok(model)
    }

    /// The price of an available route, when the catalogue states one.
    pub fn price(&self, id: &str) -> Option<&ModelPrice> {
        self.models
            .iter()
            .find(|model| model.id == id && model.available)
            .and_then(|model| model.price.as_ref())
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
        match matches.as_slice() {
            [] => Ok(None),
            [only] => Ok(Some(*only)),
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

/// A catalogue is checked for what every reader relies on: non-empty,
/// distinct model ids and stated prices that are finite and not negative.
/// Only a catalogue Brama served carries Brama's protocol version; a direct
/// OpenAI-compatible provider negotiates no Brama schema and may state a
/// version of its own, which is not Brama's to judge.
pub(super) fn validate_catalog(catalog: &ModelCatalog, via_brama: bool) -> Result<(), BramaError> {
    if via_brama && catalog.version != API_VERSION {
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
        // Only a stated side is checked; an unstated one is not invalid, it
        // is unpriced, and every reader of it says so.
        if let Some(price) = &model.price {
            for (side, rate) in price.sides() {
                // Comparing with magnitude preserves signed zero as nonnegative.
                if let Some(rate) = rate.filter(|rate| !rate.is_finite() || *rate < rate.abs()) {
                    return Err(BramaError::InvalidCatalog(format!(
                        "invalid {side} price {rate} for `{}`",
                        model.id
                    )));
                }
            }
        }
    }
    Ok(())
}
