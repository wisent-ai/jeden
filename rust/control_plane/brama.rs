use super::{now_ms, ServiceHealth};
use reqwest::blocking::Client;
use reqwest::StatusCode;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::io::Read;
use std::sync::{LazyLock, Mutex};
use std::time::{Duration, Instant};

const API_VERSION: &str = "v1";
const DEFAULT_TTL: Duration = Duration::from_secs(300);
const MAX_CACHES: usize = 8;
const MAX_RESPONSE_BYTES: u64 = 4 * 1024 * 1024;

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ModelPrice {
    #[serde(default)] pub input: f64,
    #[serde(default)] pub output: f64,
    #[serde(default)] pub cache_read: f64,
    #[serde(default)] pub cache_write: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ModelEntry {
    pub id: String,
    #[serde(default = "default_true")] pub available: bool,
    #[serde(default)] pub context_window: u64,
    #[serde(default)] pub max_output_tokens: u64,
    #[serde(default)] pub input_modalities: Vec<String>,
    #[serde(default)] pub output_modalities: Vec<String>,
    #[serde(default)] pub tools: bool,
    #[serde(default)] pub reasoning: bool,
    #[serde(default, alias = "cost", alias = "pricing")] pub price: ModelPrice,
    #[serde(default)] pub fallback: Vec<String>,
    #[serde(default)] pub promotion: Vec<String>,
    #[serde(default)] pub unavailable_reason: Option<String>,
}

fn default_true() -> bool { true }

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ModelCatalog {
    #[serde(default = "api_version")] pub version: String,
    #[serde(default)] pub models: Vec<ModelEntry>,
}
fn api_version() -> String { API_VERSION.into() }

impl ModelCatalog {
    pub(crate) fn resolve(&self, id: &str) -> Result<&ModelEntry, BramaError> {
        let model = self.models.iter().find(|model| model.id == id)
            .ok_or_else(|| BramaError::UnknownModel(id.to_string()))?;
        if !model.available {
            return Err(BramaError::UnavailableModel { model: id.to_string(), reason: model.unavailable_reason.clone().unwrap_or_else(|| "catalog marks route unavailable".into()) });
        }
        Ok(model)
    }

    pub(crate) fn price(&self, id: &str) -> Option<&ModelPrice> {
        self.models.iter().find(|model| model.id == id && model.available).map(|model| &model.price)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum BramaError {
    Unconfigured,
    Transport(String),
    Http { status: u16, message: String },
    InvalidCatalog(String),
    UnknownModel(String),
    UnavailableModel { model: String, reason: String },
}
impl std::fmt::Display for BramaError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Unconfigured => f.write_str("Brama service endpoint is not configured"),
            Self::Transport(e) => write!(f, "Brama transport error: {e}"),
            Self::Http { status, message } => write!(f, "Brama returned HTTP {status}: {message}"),
            Self::InvalidCatalog(e) => write!(f, "invalid Brama catalog: {e}"),
            Self::UnknownModel(id) => write!(f, "model `{id}` is not in the Brama catalog"),
            Self::UnavailableModel { model, reason } => write!(f, "model `{model}` is unavailable: {reason}"),
        }
    }
}

#[derive(Clone)]
pub(crate) struct BramaClient {
    endpoint: Option<String>,
    bearer: Option<String>,
    ttl: Duration,
    http: Client,
}

#[derive(Clone)]
struct CachedCatalog { catalog: ModelCatalog, etag: Option<String>, fetched: Instant }
static CACHE: LazyLock<Mutex<HashMap<String, CachedCatalog>>> = LazyLock::new(|| Mutex::new(HashMap::new()));

impl BramaClient {
    pub(crate) fn from_env() -> Self {
        Self::configured(std::env::var("BRAMA_URL").ok(), std::env::var("BRAMA_TOKEN").ok())
    }

    pub(crate) fn configured(endpoint: Option<String>, bearer: Option<String>) -> Self {
        let ttl = std::env::var("BRAMA_CATALOG_TTL_MS").ok().and_then(|value| value.parse::<u64>().ok()).filter(|value| *value > 0).map(Duration::from_millis).unwrap_or(DEFAULT_TTL);
        Self::new(endpoint, bearer, ttl)
    }

    pub(crate) fn new(endpoint: Option<String>, bearer: Option<String>, ttl: Duration) -> Self {
        let endpoint = endpoint.map(|v| v.trim_end_matches('/').to_string()).filter(|v| !v.is_empty());
        let http = Client::builder().connect_timeout(Duration::from_secs(5)).timeout(Duration::from_secs(20)).build().unwrap_or_else(|_| Client::new());
        Self { endpoint, bearer: bearer.filter(|v| !v.trim().is_empty()), ttl: if ttl.is_zero() { DEFAULT_TTL } else { ttl }, http }
    }

    fn key(&self) -> Result<String, BramaError> {
        self.endpoint.clone().ok_or(BramaError::Unconfigured)
    }

    pub(crate) fn health(&self) -> ServiceHealth {
        let available = self.endpoint.is_some();
        ServiceHealth { service: "brama".into(), version: API_VERSION.into(), available, endpoint: self.endpoint.clone(), detail: if available { "configured; catalog is resolved on demand".into() } else { "BRAMA_URL is not configured".into() }, checked_at_ms: now_ms() }
    }

    pub(crate) fn invalidate(&self) {
        if let Some(endpoint) = &self.endpoint { if let Ok(mut cache) = CACHE.lock() { cache.remove(endpoint); } }
    }

    pub(crate) fn invalidate_all() {
        if let Ok(mut cache) = CACHE.lock() { cache.clear(); }
    }

    pub(crate) fn catalog(&self, force: bool) -> Result<ModelCatalog, BramaError> {
        let key = self.key()?;
        let prior = CACHE.lock().map_err(|_| BramaError::Transport("catalog cache lock poisoned".into()))?.get(&key).cloned();
        if !force { if let Some(cached) = &prior { if cached.fetched.elapsed() < self.ttl { return Ok(cached.catalog.clone()); } } }
        let mut request = self.http.get(format!("{key}/{API_VERSION}/models"));
        if let Some(token) = &self.bearer { request = request.bearer_auth(token); }
        if let Some(etag) = prior.as_ref().and_then(|entry| entry.etag.as_ref()) { request = request.header("if-none-match", etag); }
        let response = request.send().map_err(|e| BramaError::Transport(e.to_string()))?;
        if response.status() == StatusCode::NOT_MODIFIED {
            let mut cached = prior.ok_or_else(|| BramaError::InvalidCatalog("304 without a cached catalog".into()))?;
            cached.fetched = Instant::now();
            let catalog = cached.catalog.clone();
            CACHE.lock().map_err(|_| BramaError::Transport("catalog cache lock poisoned".into()))?.insert(key, cached);
            return Ok(catalog);
        }
        let status = response.status();
        let etag = response.headers().get("etag").and_then(|v| v.to_str().ok()).map(str::to_string);
        if response.content_length().is_some_and(|length| length > MAX_RESPONSE_BYTES) { return Err(BramaError::InvalidCatalog("response exceeds 4 MiB".into())); }
        let mut body = Vec::new();
        std::io::Read::take(response, MAX_RESPONSE_BYTES + 1).read_to_end(&mut body).map_err(|e| BramaError::Transport(e.to_string()))?;
        if body.len() as u64 > MAX_RESPONSE_BYTES { return Err(BramaError::InvalidCatalog("response exceeds 4 MiB".into())); }
        let text = String::from_utf8(body).map_err(|e| BramaError::InvalidCatalog(e.to_string()))?;
        if !status.is_success() { return Err(BramaError::Http { status: status.as_u16(), message: text.chars().take(800).collect() }); }
        let catalog: ModelCatalog = serde_json::from_str(&text).map_err(|e| BramaError::InvalidCatalog(e.to_string()))?;
        validate_catalog(&catalog)?;
        let mut cache = CACHE.lock().map_err(|_| BramaError::Transport("catalog cache lock poisoned".into()))?;
        if cache.len() >= MAX_CACHES && !cache.contains_key(&key) { if let Some(oldest) = cache.iter().min_by_key(|(_, value)| value.fetched).map(|(key, _)| key.clone()) { cache.remove(&oldest); } }
        cache.insert(key, CachedCatalog { catalog: catalog.clone(), etag, fetched: Instant::now() });
        Ok(catalog)
    }
}

fn validate_catalog(catalog: &ModelCatalog) -> Result<(), BramaError> {
    if catalog.version != API_VERSION { return Err(BramaError::InvalidCatalog(format!("unsupported version `{}`", catalog.version))); }
    let mut ids = std::collections::HashSet::with_capacity(catalog.models.len());
    for model in &catalog.models {
        if model.id.trim().is_empty() { return Err(BramaError::InvalidCatalog("model id is empty".into())); }
        if !ids.insert(&model.id) { return Err(BramaError::InvalidCatalog(format!("duplicate model `{}`", model.id))); }
        if !model.price.input.is_finite() || !model.price.output.is_finite() || model.price.input < 0.0 || model.price.output < 0.0 { return Err(BramaError::InvalidCatalog(format!("invalid price for `{}`", model.id))); }
    }
    Ok(())
}
