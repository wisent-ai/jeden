use super::contract::{ModelRequest, ModelStreamResultV1, RequestMeta, RouteRequest};
use super::transport::{
    ControlPlaneTransport, ReqwestTransport, SecretRef, TransportRequest, TransportResponse,
};
use super::{now_ms, ServiceHealth};
use reqwest::StatusCode;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashMap};
use std::sync::{Arc, LazyLock, Mutex};
use std::time::{Duration, Instant};

const API_VERSION: &str = "v1";
const DEFAULT_TTL: Duration = Duration::from_secs(300);
const MAX_CACHES: usize = 8;
const MAX_RESPONSE_BYTES: u64 = 4 * 1024 * 1024;

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

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BramaError {
    Unconfigured,
    Transport(String),
    Http { status: u16, message: String },
    RateLimited { retry_after_ms: Option<u64> },
    InvalidCatalog(String),
    InvalidResponse(String),
    UnknownModel(String),
    UnavailableModel { model: String, reason: String },
    Cancelled,
}
impl std::fmt::Display for BramaError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Unconfigured => f.write_str(
                "BRAMA_URL is required; configure the Brama model-router service URL",
            ),
            Self::Transport(e) => write!(f, "Brama transport error: {e}"),
            Self::Http { status, message } => write!(f, "Brama returned HTTP {status}: {message}"),
            Self::RateLimited { retry_after_ms } => write!(
                f,
                "Brama rate limited the request; retry after {:?} ms",
                retry_after_ms
            ),
            Self::InvalidCatalog(e) => write!(f, "invalid Brama catalog: {e}"),
            Self::InvalidResponse(e) => write!(f, "invalid Brama response: {e}"),
            Self::UnknownModel(id) => write!(f, "model `{id}` is not in the Brama catalog"),
            Self::UnavailableModel { model, reason } => {
                write!(f, "model `{model}` is unavailable: {reason}")
            }
            Self::Cancelled => f.write_str("Brama request cancelled"),
        }
    }
}

#[derive(Clone)]
pub struct BramaClient {
    endpoint: Option<String>,
    authorization: Option<SecretRef>,
    ttl: Duration,
    transport: Arc<dyn ControlPlaneTransport>,
    correlation: Arc<std::sync::atomic::AtomicU64>,
}

#[derive(Clone)]
struct CachedCatalog {
    catalog: ModelCatalog,
    etag: Option<String>,
    fetched: Instant,
}
static CACHE: LazyLock<Mutex<HashMap<String, CachedCatalog>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

/// On-disk catalog cache shared across processes: `~/.jeden/cache/`
/// `brama-models-<sha256(endpoint + bearer + agent scope)[:16]>.json`. The
/// caller scope is part of the key because Brama filters and discovers models
/// for the signed agent. The in-memory CACHE above only lives for one process,
/// so a fresh `jeden` start can reuse the matching scoped catalog.
fn disk_cache_path(key: &str) -> std::path::PathBuf {
    use sha2::Digest;
    let digest = sha2::Sha256::digest(key.as_bytes());
    crate::dirs_home().join(format!(
        ".jeden/cache/brama-models-{}.json",
        hex::encode(digest)[..16].to_string()
    ))
}

fn read_disk_cache(key: &str) -> Option<(ModelCatalog, Option<String>, u64)> {
    let text = std::fs::read_to_string(disk_cache_path(key)).ok()?;
    let value: serde_json::Value = serde_json::from_str(&text).ok()?;
    let fetched_ms = value.get("fetchedAtMs")?.as_u64()?;
    let etag = value
        .get("etag")
        .and_then(serde_json::Value::as_str)
        .map(str::to_string);
    let catalog: ModelCatalog = serde_json::from_value(value.get("catalog")?.clone()).ok()?;
    Some((catalog, etag, fetched_ms))
}

fn write_disk_cache(key: &str, catalog: &ModelCatalog, etag: Option<&str>) {
    let path = disk_cache_path(key);
    let Some(parent) = path.parent() else {
        return;
    };
    if std::fs::create_dir_all(parent).is_err() {
        return;
    }
    let payload = serde_json::json!({
        "fetchedAtMs": now_ms(),
        "etag": etag,
        "catalog": catalog,
    });
    // tmp + rename keeps the entry atomic for concurrent jeden processes.
    let tmp = path.with_extension("json.tmp");
    if std::fs::write(&tmp, payload.to_string()).is_ok() {
        let _ = std::fs::rename(&tmp, &path);
    }
}

fn caller_credentials() -> Option<(String, String)> {
    let secret = std::env::var("WISENT_APP_AGENT_AUTH_SECRET")
        .ok()
        .filter(|value| !value.is_empty())?;
    let agent_id = std::env::var("WISENT_APP_AGENT_ID")
        .ok()
        .filter(|value| !value.trim().is_empty())
        .unwrap_or_else(|| "wisent-app".into());
    Some((agent_id, secret))
}

fn insert_caller_auth_headers(headers: &mut BTreeMap<String, String>, body: &[u8]) {
    let Some((agent_id, secret)) = caller_credentials() else {
        return;
    };
    let Ok(body) = std::str::from_utf8(body) else {
        return;
    };
    let Ok((timestamp, body_hash, signature)) =
        crate::model_router::hmac_headers(body, &agent_id, &secret)
    else {
        return;
    };
    headers.insert("x-agent-id".into(), agent_id);
    headers.insert("x-agent-timestamp".into(), timestamp);
    headers.insert("x-agent-body-sha256".into(), body_hash);
    headers.insert("x-agent-signature".into(), signature);
}

fn catalog_cache_key(endpoint: &str, authorization: Option<&SecretRef>) -> String {
    use sha2::Digest;
    let bearer_scope = authorization
        .and_then(SecretRef::resolve)
        .map(|token| hex::encode(sha2::Sha256::digest(token.as_bytes())))
        .unwrap_or_else(|| "anonymous".into());
    let agent_scope = caller_credentials()
        .map(|(agent_id, _)| agent_id)
        .unwrap_or_else(|| "unsigned".into());
    format!("{endpoint}\u{0}bearer={bearer_scope}\u{0}agent={agent_scope}")
}

impl BramaClient {
    pub fn from_env() -> Self {
        Self::with_secret_ref(
            std::env::var("BRAMA_URL")
                .ok()
                .filter(|value| !value.trim().is_empty()),
            Some(SecretRef::environment("BRAMA_TOKEN")),
            DEFAULT_TTL,
            ReqwestTransport::production(),
        )
    }

    pub fn configured(endpoint: Option<String>, bearer: Option<String>) -> Self {
        let ttl = std::env::var("BRAMA_CATALOG_TTL_MS")
            .ok()
            .and_then(|value| value.parse::<u64>().ok())
            .filter(|value| *value > 0)
            .map(Duration::from_millis)
            .unwrap_or(DEFAULT_TTL);
        Self::new(endpoint, bearer, ttl)
    }

    pub fn new(endpoint: Option<String>, bearer: Option<String>, ttl: Duration) -> Self {
        Self::with_transport(endpoint, bearer, ttl, ReqwestTransport::production())
    }

    pub fn with_transport(
        endpoint: Option<String>,
        bearer: Option<String>,
        ttl: Duration,
        transport: Arc<dyn ControlPlaneTransport>,
    ) -> Self {
        Self::with_secret_ref(endpoint, bearer.map(SecretRef::inline), ttl, transport)
    }

    pub fn with_secret_ref(
        endpoint: Option<String>,
        authorization: Option<SecretRef>,
        ttl: Duration,
        transport: Arc<dyn ControlPlaneTransport>,
    ) -> Self {
        let endpoint = endpoint
            .map(|v| v.trim_end_matches('/').to_string())
            .filter(|v| !v.is_empty());
        Self {
            endpoint,
            authorization,
            ttl: if ttl.is_zero() { DEFAULT_TTL } else { ttl },
            transport,
            correlation: Arc::new(std::sync::atomic::AtomicU64::new(1)),
        }
    }

    fn key(&self) -> Result<String, BramaError> {
        self.endpoint.clone().ok_or(BramaError::Unconfigured)
    }

    pub fn health(&self) -> ServiceHealth {
        let available = self.endpoint.is_some();
        ServiceHealth {
            service: "brama".into(),
            version: API_VERSION.into(),
            available,
            endpoint: self.endpoint.clone(),
            detail: if available {
                "configured; catalog is resolved on demand".into()
            } else {
                "BRAMA_URL is required; configure the Brama model-router service URL".into()
            },
            checked_at_ms: now_ms(),
        }
    }

    pub fn invalidate(&self) {
        if let Some(endpoint) = &self.endpoint {
            let key = catalog_cache_key(endpoint, self.authorization.as_ref());
            if let Ok(mut cache) = CACHE.lock() {
                cache.remove(&key);
            }
        }
    }

    pub fn invalidate_all() {
        if let Ok(mut cache) = CACHE.lock() {
            cache.clear();
        }
    }

    pub fn catalog(&self, force: bool) -> Result<ModelCatalog, BramaError> {
        let endpoint = self.key()?;
        let key = catalog_cache_key(&endpoint, self.authorization.as_ref());
        let prior = CACHE
            .lock()
            .map_err(|_| BramaError::Transport("catalog cache lock poisoned".into()))?
            .get(&key)
            .cloned();
        if !force {
            if let Some(cached) = &prior {
                if cached.fetched.elapsed() < self.ttl {
                    return Ok(cached.catalog.clone());
                }
            }
            // Cold process: hydrate from the on-disk catalog when it is still
            // inside the TTL, so a fresh `jeden` start skips the network
            // entirely (the in-memory cache above only covers one process).
            if let Some((catalog, etag, fetched_ms)) = read_disk_cache(&key) {
                if now_ms().saturating_sub(fetched_ms) < self.ttl.as_millis() as u64 {
                    CACHE
                        .lock()
                        .map_err(|_| BramaError::Transport("catalog cache lock poisoned".into()))?
                        .insert(
                            key.clone(),
                            CachedCatalog {
                                catalog: catalog.clone(),
                                etag,
                                fetched: Instant::now(),
                            },
                        );
                    return Ok(catalog);
                }
            }
        }
        // A stale disk entry still seeds the conditional request: on 304 we
        // rehydrate from disk instead of downloading the full catalog.
        let disk_prior = if prior.is_none() {
            read_disk_cache(&key)
        } else {
            None
        };
        let mut headers = BTreeMap::new();
        headers.insert("x-jeden-schema-min".into(), "1".into());
        headers.insert("x-jeden-schema-max".into(), "1".into());
        headers.insert(
            "x-correlation-id".into(),
            format!(
                "brama-{}",
                self.correlation
                    .fetch_add(1, std::sync::atomic::Ordering::Relaxed)
            ),
        );
        if let Some(token) = self.authorization.as_ref().and_then(SecretRef::resolve) {
            headers.insert("authorization".into(), format!("Bearer {token}"));
        }
        insert_caller_auth_headers(&mut headers, &[]);
        if let Some(etag) = prior
            .as_ref()
            .and_then(|entry| entry.etag.as_ref())
            .or(disk_prior.as_ref().and_then(|entry| entry.1.as_ref()))
        {
            headers.insert("if-none-match".into(), etag.clone());
        }
        let response = match self.transport.execute(TransportRequest {
            method: reqwest::Method::GET,
            url: format!("{endpoint}/{API_VERSION}/models"),
            headers,
            body: None,
            max_response_bytes: MAX_RESPONSE_BYTES,
        }) {
            Ok(response) => response,
            Err(error) => {
                if let Some(mut cached) = prior.map(|entry| entry.catalog) {
                    cached.degraded = true;
                    return Ok(cached);
                }
                if let Some((mut catalog, _, _)) = disk_prior {
                    catalog.degraded = true;
                    return Ok(catalog);
                }
                return Err(BramaError::Transport(error));
            }
        };
        super::contract::negotiate_response(&response.headers).map_err(|error| {
            BramaError::InvalidResponse(format!("schema negotiation failed: {error:?}"))
        })?;
        if response.status == StatusCode::NOT_MODIFIED.as_u16() {
            let mut cached = prior.or_else(|| {
                disk_prior.map(|(catalog, etag, _)| CachedCatalog {
                    catalog,
                    etag,
                    fetched: Instant::now(),
                })
            });
            let mut cached = cached
                .take()
                .ok_or_else(|| BramaError::InvalidCatalog("304 without a cached catalog".into()))?;
            cached.fetched = Instant::now();
            let catalog = cached.catalog.clone();
            write_disk_cache(&key, &cached.catalog, cached.etag.as_deref());
            CACHE
                .lock()
                .map_err(|_| BramaError::Transport("catalog cache lock poisoned".into()))?
                .insert(key, cached);
            return Ok(catalog);
        }
        let status = response.status;
        let etag = response.headers.get("etag").cloned();
        let text = String::from_utf8(response.body)
            .map_err(|e| BramaError::InvalidCatalog(e.to_string()))?;
        if !(200..300).contains(&status) {
            return Err(BramaError::Http {
                status,
                message: "request failed; response body suppressed".into(),
            });
        }
        let mut catalog: ModelCatalog =
            serde_json::from_str(&text).map_err(|e| BramaError::InvalidCatalog(e.to_string()))?;
        if catalog.catalog_revision.is_empty() {
            catalog.catalog_revision = response
                .headers
                .get("x-catalog-revision")
                .cloned()
                .or_else(|| etag.clone())
                .unwrap_or_else(|| catalog.version.clone());
        }
        validate_catalog(&catalog)?;
        write_disk_cache(&key, &catalog, etag.as_deref());
        let mut cache = CACHE
            .lock()
            .map_err(|_| BramaError::Transport("catalog cache lock poisoned".into()))?;
        if cache.len() >= MAX_CACHES && !cache.contains_key(&key) {
            if let Some(oldest) = cache
                .iter()
                .min_by_key(|(_, value)| value.fetched)
                .map(|(key, _)| key.clone())
            {
                cache.remove(&oldest);
            }
        }
        cache.insert(
            key,
            CachedCatalog {
                catalog: catalog.clone(),
                etag,
                fetched: Instant::now(),
            },
        );
        Ok(catalog)
    }
    fn request_json(
        &self,
        method: reqwest::Method,
        path: &str,
        body: Option<Vec<u8>>,
        meta: &RequestMeta,
    ) -> Result<TransportResponse, BramaError> {
        super::contract::negotiate(meta.schema_min, meta.schema_max).map_err(|error| {
            BramaError::InvalidResponse(format!("schema negotiation failed: {error:?}"))
        })?;
        let mut headers = BTreeMap::new();
        headers.insert("x-jeden-schema-min".into(), meta.schema_min.to_string());
        headers.insert("x-jeden-schema-max".into(), meta.schema_max.to_string());
        headers.insert("x-correlation-id".into(), meta.correlation_id.clone());
        if let Some(key) = &meta.idempotency_key {
            headers.insert("idempotency-key".into(), key.clone());
        }
        if let Some(token) = self.authorization.as_ref().and_then(SecretRef::resolve) {
            headers.insert("authorization".into(), format!("Bearer {token}"));
        }
        insert_caller_auth_headers(&mut headers, body.as_deref().unwrap_or_default());
        let response = self
            .transport
            .execute(TransportRequest {
                method,
                url: format!("{}/{API_VERSION}{path}", self.key()?),
                headers,
                body,
                max_response_bytes: MAX_RESPONSE_BYTES,
            })
            .map_err(BramaError::Transport)?;
        super::contract::negotiate_response(&response.headers).map_err(|error| {
            BramaError::InvalidResponse(format!("schema negotiation failed: {error:?}"))
        })?;
        if response.status == 429 {
            let retry_after_ms = response
                .headers
                .get("retry-after")
                .and_then(|value| value.parse::<u64>().ok())
                .map(|seconds| seconds.saturating_mul(1000));
            return Err(BramaError::RateLimited { retry_after_ms });
        }
        if !(200..300).contains(&response.status) {
            return Err(BramaError::Http {
                status: response.status,
                message: "request failed; response body suppressed".into(),
            });
        }
        Ok(response)
    }
}

fn validate_catalog(catalog: &ModelCatalog) -> Result<(), BramaError> {
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

impl super::contract::BramaApiV1 for BramaClient {
    fn health(&self) -> ServiceHealth {
        BramaClient::health(self)
    }

    fn readiness(&self) -> Result<super::contract::Readiness, BramaError> {
        let catalog = self.catalog(false)?;
        super::contract::negotiate(1, 1).map_err(|error| {
            BramaError::InvalidCatalog(format!("schema negotiation failed: {error:?}"))
        })?;
        Ok(super::contract::Readiness {
            ready: true,
            schema_min: 1,
            schema_max: 1,
            max_payload_bytes: MAX_RESPONSE_BYTES,
            detail: format!(
                "{} routes available",
                catalog
                    .models
                    .iter()
                    .filter(|model| model.available)
                    .count()
            ),
        })
    }

    fn capabilities(&self, meta: &RequestMeta) -> Result<Vec<String>, BramaError> {
        let response = self.request_json(reqwest::Method::GET, "/capabilities", None, meta)?;
        let value: serde_json::Value = serde_json::from_slice(&response.body)
            .map_err(|error| BramaError::InvalidResponse(error.to_string()))?;
        serde_json::from_value(value.get("capabilities").cloned().unwrap_or(value))
            .map_err(|error| BramaError::InvalidResponse(error.to_string()))
    }

    fn catalog(&self, force: bool) -> Result<ModelCatalog, BramaError> {
        BramaClient::catalog(self, force)
    }

    fn resolve(
        &self,
        request: &RouteRequest,
        meta: &RequestMeta,
    ) -> Result<ModelEntry, BramaError> {
        let body = serde_json::to_vec(request)
            .map_err(|error| BramaError::InvalidResponse(error.to_string()))?;
        let response = self.request_json(reqwest::Method::POST, "/resolve", Some(body), meta)?;
        let route: ModelEntry = serde_json::from_slice(&response.body)
            .map_err(|error| BramaError::InvalidResponse(error.to_string()))?;
        if route.id.trim().is_empty() {
            return Err(BramaError::InvalidResponse("served route is empty".into()));
        }
        Ok(route)
    }

    fn stream(
        &self,
        request: &ModelRequest,
        meta: &RequestMeta,
        cancelled: &dyn Fn() -> bool,
    ) -> Result<ModelStreamResultV1, BramaError> {
        if cancelled() {
            return Err(BramaError::Cancelled);
        }
        let body = serde_json::to_vec(request)
            .map_err(|error| BramaError::InvalidResponse(error.to_string()))?;
        let response = self.request_json(reqwest::Method::POST, "/stream", Some(body), meta)?;
        if cancelled() {
            return Err(BramaError::Cancelled);
        }
        let mut result: ModelStreamResultV1 = serde_json::from_slice(&response.body)
            .map_err(|error| BramaError::InvalidResponse(error.to_string()))?;
        if result.served_route.trim().is_empty()
            || result.finish_reason.trim().is_empty()
            || result.correlation_id != meta.correlation_id
        {
            return Err(BramaError::InvalidResponse(
                "stream terminal metadata is incomplete or correlation mismatched".into(),
            ));
        }
        if result.selected_route.is_empty() {
            result.selected_route = request.route.clone();
        }
        if result.selected_route != request.route {
            return Err(BramaError::InvalidResponse(
                "selected route does not match the requested route".into(),
            ));
        }
        if let Some(snapshot) = &result.billing {
            if snapshot.provider_id.is_empty()
                || snapshot.account_id.is_empty()
                || snapshot.subscription_id.is_empty()
                || snapshot.quota.subscription_id != snapshot.subscription_id
            {
                return Err(BramaError::InvalidResponse(
                    "billing attribution is incomplete or mismatched".into(),
                ));
            }
        }
        Ok(result)
    }
}
