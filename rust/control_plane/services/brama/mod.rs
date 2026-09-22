use super::contract::{ModelRequest, ModelStreamResultV1, RequestMeta, RouteRequest};
use super::transport::{ControlPlaneTransport, ReqwestTransport, SecretRef};
use super::{now_ms, ServiceHealth};
use reqwest::StatusCode;
use serde_json::Value;
use std::sync::Arc;
use std::time::Duration;

pub(super) const API_VERSION: &str = "v1";
const DEFAULT_TTL: Duration = Duration::from_secs(300);
pub(super) const MAX_CACHES: usize = 8;
pub(super) const MAX_RESPONSE_BYTES: u64 = 4 * 1024 * 1024;

mod auth;
mod cache;
mod catalog;
mod fetch;

use auth::insert_caller_auth_headers;
use cache::{CACHE, catalog_cache_key};
pub use catalog::{BramaReadiness, ModelCatalog, ModelEntry, ModelPerf, ModelPrice};
use catalog::validate_catalog;

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
            Self::Unconfigured => {
                f.write_str("BRAMA_URL is required; configure the Brama model-router service URL")
            }
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
    pub(super) endpoint: Option<String>,
    pub(super) authorization: Option<SecretRef>,
    pub(super) ttl: Duration,
    transport: Arc<dyn ControlPlaneTransport>,
    pub(super) correlation: Arc<std::sync::atomic::AtomicU64>,
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

    pub(super) fn key(&self) -> Result<String, BramaError> {
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
