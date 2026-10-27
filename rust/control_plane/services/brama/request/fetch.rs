//! Catalog retrieval and cache policy for Brama and direct providers.
//!
//! Split out of `control_plane/services/brama.rs`, which had grown past the
//! module line cap.

use super::super::transport::{SecretRef, TransportRequest};
use super::cache::{catalog_cache_key, read_disk_cache, write_disk_cache, CachedCatalog, CACHE};
use super::catalog::validate_catalog;
use super::{BramaClient, BramaError, ModelCatalog};
use crate::control_plane::services::brama::auth::insert_caller_auth_headers;
use crate::control_plane::services::brama::API_VERSION;
use reqwest::StatusCode;
use sha2::Digest;
use std::collections::BTreeMap;
use std::time::Instant;

impl BramaClient {
    pub fn catalog(&self, force: bool) -> Result<ModelCatalog, BramaError> {
        let endpoint = self.key()?;
        let direct =
            crate::agent::credential::direct_provider().is_some_and(|(url, _)| url == endpoint);
        // A direct provider's authorization may rotate between requests.
        // An ETag and catalog saved under one token cannot validate a later
        // request made with another token or no token.
        let authorization = SecretRef::resolve_optional(self.authorization.as_ref())
            .map_err(BramaError::Configuration)?;
        let key = catalog_cache_key(&endpoint, authorization.as_deref()).map_err(BramaError::Configuration)?;
        let prior = if direct {
            None
        } else {
            // Catalog entries are inserted whole: a holder that panicked left a
            // cache, not a reason to call Brama's model catalog unusable.
            crate::panicked::held(&CACHE)
                .get(&key)
                .cloned()
        };
        if !force && !direct {
            if let Some(cached) = &prior {
                if self.ttl.is_some_and(|ttl| cached.fetched.elapsed() < ttl) {
                    return Ok(cached.catalog.clone());
                }
            }
            // Cold process: hydrate from the on-disk catalog when it is still
            // inside the TTL, so a fresh `jeden` start skips the network
            // entirely (the in-memory cache above only covers one process).
            if let Some((catalog, etag, fetched_ms)) = read_disk_cache(&key) {
                // The kept catalogue's age is measured on the clock; a clock
                // that cannot be read cannot call it fresh, so the catalogue
                // is fetched again and the reason is said.
                let fresh = match crate::clock::unix_millis() {
                    Ok(now) => self
                        .ttl
                        .is_some_and(|ttl| now.saturating_sub(fetched_ms) < ttl.as_millis() as u64),
                    Err(error) => {
                        eprintln!(
                            "jeden: the kept model catalogue's age cannot be measured: {error}"
                        );
                        false
                    }
                };
                if fresh {
                    crate::panicked::held(&CACHE)
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
        let disk_prior = if direct || prior.is_some() {
            None
        } else {
            read_disk_cache(&key)
        };
        let mut headers = BTreeMap::new();
        // Direct requests neither construct nor send Brama's signed headers.
        if !direct {
            headers.insert(
                "x-jeden-schema-min".into(),
                super::super::contract::MIN_SCHEMA_VERSION.to_string(),
            );
            headers.insert(
                "x-jeden-schema-max".into(),
                super::super::contract::MIN_SCHEMA_VERSION.to_string(),
            );
            headers.insert(
                "x-correlation-id".into(),
                format!("brama-{}", crate::tally::draw(&self.correlation)),
            );
            insert_caller_auth_headers(&mut headers, &[]).map_err(BramaError::Configuration)?;
        }
        if let Some(token) = authorization {
            headers.insert("authorization".into(), format!("Bearer {token}"));
        }
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
        }) {
            Ok(response) => response,
            Err(error) => {
                if direct {
                    return Err(BramaError::Transport(format!(
                        "direct provider GET {endpoint}/{API_VERSION}/models failed: {error}"
                    )));
                }
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
        if !direct {
            super::super::contract::negotiate_response(&response.headers).map_err(|error| {
                BramaError::InvalidResponse(format!("schema negotiation failed: {error:?}"))
            })?;
        }
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
            crate::panicked::held(&CACHE)
                .insert(key, cached);
            return Ok(catalog);
        }
        let status = response.status;
        if !StatusCode::from_u16(status).is_ok_and(|status| status.is_success()) {
            return Err(BramaError::http(status, "/models", &response.body));
        }
        let etag = response.headers.get("etag").cloned();
        let text = String::from_utf8(response.body)
            .map_err(|e| BramaError::InvalidCatalog(e.to_string()))?;
        let mut catalog: ModelCatalog =
            serde_json::from_str(&text).map_err(|e| BramaError::InvalidCatalog(e.to_string()))?;
        if catalog.catalog_revision.is_empty() {
            catalog.catalog_revision = if direct {
                format!(
                    "sha256:{}",
                    hex::encode(sha2::Sha256::digest(text.as_bytes()))
                )
            } else {
                response
                    .headers
                    .get("x-catalog-revision")
                    .cloned()
                    .or_else(|| etag.clone())
                    .or_else(|| (!catalog.version.is_empty()).then(|| catalog.version.clone()))
                    .ok_or_else(|| {
                        BramaError::InvalidCatalog(
                            "model catalog has no revision header, ETag, or version".into(),
                        )
                    })?
            };
        }
        validate_catalog(&catalog, !direct)?;
        if !direct {
            write_disk_cache(&key, &catalog, etag.as_deref());
            let mut cache = crate::panicked::held(&CACHE);
            cache.insert(
                key,
                CachedCatalog {
                    catalog: catalog.clone(),
                    etag,
                    fetched: Instant::now(),
                },
            );
        }
        Ok(catalog)
    }
}
