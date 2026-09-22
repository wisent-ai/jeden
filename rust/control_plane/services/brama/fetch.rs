//! Asking the gateway for its readiness and its catalogue, and deciding when
//! the answer already in hand is good enough.
//!
//! Split out of `control_plane/services/brama.rs`, which had grown past the
//! module line cap.

use super::super::contract::RequestMeta;
use super::super::transport::{SecretRef, TransportRequest, TransportResponse};
use super::cache::{catalog_cache_key, read_disk_cache, write_disk_cache, CachedCatalog, CACHE};
use super::catalog::validate_catalog;
use super::{
    BramaClient, BramaError, BramaReadiness, ModelCatalog, MAX_CACHES, MAX_RESPONSE_BYTES,
};
use crate::control_plane::now_ms;
use crate::control_plane::services::brama::auth::insert_caller_auth_headers;
use crate::control_plane::services::brama::API_VERSION;
use reqwest::StatusCode;
use serde_json::Value;
use std::collections::BTreeMap;
use std::time::Instant;

impl BramaClient {
    ///
    /// It is not under `/v1`, so it does not go through the versioned request
    /// helper and negotiates no schema headers.
    /// What Brama says about its own readiness, which is the only answer that
    /// covers credentials: `/health` reports that a process exists and
    /// `/v1/models` that a catalog can be listed, while a subscription whose
    /// credential the provider rejects appears in neither. This route redeems
    /// one credential per configured provider and answers `degraded` with its
    /// own sentence, which is the state `jeden run` then fails inside.
    pub fn readiness(&self) -> Result<BramaReadiness, BramaError> {
        let endpoint = self.key()?;
        let mut headers = BTreeMap::new();
        if let Some(token) = self.authorization.as_ref().and_then(SecretRef::resolve) {
            headers.insert("authorization".into(), format!("Bearer {token}"));
        }
        let response = self
            .transport
            .execute(TransportRequest {
                method: reqwest::Method::GET,
                url: format!("{endpoint}/readyz"),
                headers,
                body: None,
                max_response_bytes: MAX_RESPONSE_BYTES,
            })
            .map_err(BramaError::Transport)?;
        let value: Value = serde_json::from_slice(&response.body).map_err(|error| {
            BramaError::InvalidResponse(format!("readiness response is not JSON: {error}"))
        })?;
        let flag = |name: &str| value.get(name).and_then(Value::as_bool).unwrap_or(false);
        Ok(BramaReadiness {
            status: response.status,
            ready: flag("ready"),
            degraded: flag("degraded"),
            operator_action_required: flag("operator_action_required"),
            reason: value
                .get("reason")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string(),
            providers_without_credential: value
                .get("providers")
                .and_then(Value::as_array)
                .map(|providers| {
                    providers
                        .iter()
                        .filter(|provider| {
                            !provider
                                .get("credential")
                                .and_then(Value::as_bool)
                                .unwrap_or(false)
                        })
                        .filter_map(|provider| {
                            provider
                                .get("provider")
                                .and_then(Value::as_str)
                                .map(str::to_owned)
                        })
                        .collect()
                })
                .unwrap_or_default(),
        })
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
        super::super::contract::negotiate_response(&response.headers).map_err(|error| {
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
        if !(200..300).contains(&status) {
            return Err(BramaError::Http {
                status,
                message: format!(
                    "/{API_VERSION}/models: {:?}",
                    String::from_utf8_lossy(&response.body)
                ),
            });
        }
        let etag = response.headers.get("etag").cloned();
        let text = String::from_utf8(response.body)
            .map_err(|e| BramaError::InvalidCatalog(e.to_string()))?;
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
    pub(super) fn request_json(
        &self,
        method: reqwest::Method,
        path: &str,
        body: Option<Vec<u8>>,
        meta: &RequestMeta,
    ) -> Result<TransportResponse, BramaError> {
        super::super::contract::negotiate(meta.schema_min, meta.schema_max).map_err(|error| {
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
        super::super::contract::negotiate_response(&response.headers).map_err(|error| {
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
                message: format!(
                    "/{API_VERSION}{path}: {:?}",
                    String::from_utf8_lossy(&response.body)
                ),
            });
        }
        Ok(response)
    }
}
