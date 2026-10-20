use super::transport::{ControlPlaneTransport, ReqwestTransport, SecretRef};
use super::ServiceHealth;
use std::sync::Arc;
use std::time::Duration;

pub(super) const API_VERSION: &str = "v1";
/// Milliseconds a fetched catalog is served before it is asked for again.
/// Unset, every read revalidates with the catalog's etag, which Brama
/// answers with 304 when nothing changed.
const CATALOG_TTL_ENV: &str = "BRAMA_CATALOG_TTL_MS";

/// The declared catalog age. Missing means revalidate; an invalid value
/// stops the client before a catalog or model request.
fn declared_catalog_ttl() -> Result<Option<Duration>, String> {
    match std::env::var(CATALOG_TTL_ENV) {
        Ok(value) => value.trim().parse::<u64>()
            .map(Duration::from_millis)
            .map(Some)
            .map_err(|error| format!("{CATALOG_TTL_ENV} must be a whole number of milliseconds, got {value:?}: {error}")),
        Err(std::env::VarError::NotPresent) => Ok(None),
        Err(error) => Err(format!("{CATALOG_TTL_ENV} cannot be read: {error}")),
    }
}

#[path = "request/auth.rs"]
mod auth;
mod cache;
mod catalog;
#[path = "request/contract_impl.rs"]
mod contract_impl;
mod error;
#[path = "request/fetch.rs"]
mod fetch;
#[path = "request/media.rs"]
mod media;
mod readiness;
#[path = "request/versioned.rs"]
mod versioned;

use cache::{catalog_cache_key, CACHE};
pub use catalog::{BramaReadiness, ModelCatalog, ModelEntry, ModelPerf, ModelPrice};
pub use error::BramaError;

#[derive(Clone)]
pub struct BramaClient {
    pub(super) endpoint: Option<String>,
    pub(super) authorization: Option<SecretRef>,
    /// The declared catalog age; `None` revalidates on every read.
    pub(super) ttl: Option<Duration>,
    transport: Arc<dyn ControlPlaneTransport>,
    configuration_error: Option<String>,
    pub(super) correlation: Arc<std::sync::atomic::AtomicU64>,
}
impl BramaClient {
    pub fn from_env() -> Self {
        let mut client = Self::with_secret_ref(
            crate::agent::credential::brama_url(),
            Some(SecretRef::environment("BRAMA_TOKEN")),
            None,
            ReqwestTransport::production(),
        );
        client.apply_declared_catalog_ttl();
        client
    }

    pub fn configured(endpoint: Option<String>, bearer: Option<String>) -> Self {
        let (direct, transport) = crate::control_plane::model_transport(endpoint.as_deref());
        let mut client = Self::with_transport(endpoint, bearer, None, transport);
        if !direct {
            client.apply_declared_catalog_ttl();
        }
        client
    }

    pub fn new(endpoint: Option<String>, bearer: Option<String>, ttl: Option<Duration>) -> Self {
        let (_, transport) = crate::control_plane::model_transport(endpoint.as_deref());
        Self::with_transport(endpoint, bearer, ttl, transport)
    }

    pub fn with_transport(
        endpoint: Option<String>,
        bearer: Option<String>,
        ttl: Option<Duration>,
        transport: Arc<dyn ControlPlaneTransport>,
    ) -> Self {
        Self::with_secret_ref(
            endpoint,
            bearer
                .filter(|token| !token.is_empty())
                .map(SecretRef::inline),
            ttl,
            transport,
        )
    }

    pub fn with_secret_ref(
        endpoint: Option<String>,
        authorization: Option<SecretRef>,
        ttl: Option<Duration>,
        transport: Arc<dyn ControlPlaneTransport>,
    ) -> Self {
        let endpoint = endpoint
            .map(|v| v.trim_end_matches('/').to_string())
            .filter(|v| !v.is_empty());
        Self {
            endpoint,
            authorization,
            ttl,
            transport,
            correlation: Arc::new(std::sync::atomic::AtomicU64::default()),
            configuration_error: None,
        }
    }

    fn apply_declared_catalog_ttl(&mut self) {
        match declared_catalog_ttl() {
            Ok(ttl) => self.ttl = ttl,
            Err(error) => self.configuration_error = Some(error),
        }
    }

    pub(super) fn key(&self) -> Result<String, BramaError> {
        if let Some(cause) = crate::agent::credential::direct_refusal() {
            return Err(BramaError::Configuration(cause.to_owned()));
        }
        if let Some(error) = &self.configuration_error {
            return Err(BramaError::Configuration(error.clone()));
        }
        self.endpoint.clone().ok_or(BramaError::Unconfigured)
    }

    pub fn health(&self) -> ServiceHealth {
        let refused = crate::agent::credential::direct_refusal();
        let direct = refused.is_some()
            || crate::agent::credential::direct_provider()
                .is_some_and(|(url, _)| self.endpoint.as_deref() == Some(url.as_str()));
        let available =
            refused.is_none() && self.endpoint.is_some() && self.configuration_error.is_none();
        ServiceHealth {
            service: if direct { "model" } else { "brama" }.into(),
            version: if direct {
                String::new()
            } else {
                API_VERSION.into()
            },
            available,
            endpoint: if refused.is_some() {
                None
            } else {
                self.endpoint.clone()
            },
            detail: if let Some(cause) = refused {
                format!("direct provider route refused: {cause}")
            } else {
                match (&self.configuration_error, &self.endpoint) {
                    (Some(error), _) => format!("model endpoint configuration error: {error}"),
                    (None, Some(_)) if direct => {
                        "direct provider configured; catalog is resolved on demand".into()
                    }
                    (None, Some(_)) => "configured; catalog is resolved on demand".into(),
                    (None, None) => {
                        "BRAMA_URL is required; configure the Brama model-router service URL".into()
                    }
                }
            },
        }
    }

    pub fn invalidate(&self) {
        if let Some(endpoint) = &self.endpoint {
            let bearer = match SecretRef::resolve_optional(self.authorization.as_ref()) {
                Ok(bearer) => bearer,
                Err(error) => {
                    eprintln!("jeden: cannot invalidate model catalog cache: {error}");
                    return;
                }
            };
            let key = catalog_cache_key(endpoint, bearer.as_deref());
            match CACHE.lock() {
                Ok(mut cache) => {
                    cache.remove(&key);
                }
                Err(error) => eprintln!("jeden: cannot invalidate model catalog cache: {error}"),
            }
        }
    }

    pub fn invalidate_all() {
        match CACHE.lock() {
            Ok(mut cache) => cache.clear(),
            Err(error) => eprintln!("jeden: cannot invalidate model catalog cache: {error}"),
        }
    }
}
