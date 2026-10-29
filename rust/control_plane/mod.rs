pub mod billing;
pub mod contract;
mod services;
pub mod transport;

pub use services::{brama, quota, weles};

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::Path;
use std::sync::{Arc, LazyLock};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ServiceHealth {
    pub service: String,
    pub version: String,
    pub available: bool,
    pub endpoint: Option<String>,
    pub detail: String,
}
/// Direct provider catalogs use HTTP and bearer credentials, not Brama's
/// agent signature, schema negotiation, or internal correlation header.
struct DirectCatalogTransport {
    inner: Arc<dyn transport::ControlPlaneTransport>,
}
fn redact_catalog_response(
    mut response: transport::TransportResponse,
    secret: &str,
) -> transport::TransportResponse {
    if secret.is_empty() {
        return response;
    }
    for value in response.headers.values_mut() {
        if value.contains(secret) {
            *value = value.replace(secret, "[REDACTED]");
        }
    }
    if response
        .body
        .windows(secret.len())
        .any(|bytes| bytes == secret.as_bytes())
    {
        response.body = String::from_utf8_lossy(&response.body)
            .replace(secret, "[REDACTED]")
            .into_bytes();
    }
    response
}

pub(crate) fn model_transport(
    endpoint: Option<&str>,
) -> (bool, Arc<dyn transport::ControlPlaneTransport>) {
    let direct = crate::agent::credential::direct_provider().is_some_and(|(url, _)| {
        endpoint.is_some_and(|endpoint| endpoint.trim_end_matches('/') == url)
    });
    let transport = if direct {
        Arc::clone(&DIRECT_CATALOG_TRANSPORT)
    } else {
        transport::ReqwestTransport::production()
    };
    (direct, transport)
}

impl transport::ControlPlaneTransport for DirectCatalogTransport {
    fn execute(
        &self,
        mut request: transport::TransportRequest,
    ) -> Result<transport::TransportResponse, String> {
        request.headers.retain(|name, _| {
            !name.starts_with("x-agent-")
                && !name.starts_with("x-jeden-schema-")
                && name != "x-correlation-id"
        });
        let Some((endpoint, key)) = crate::agent::credential::direct_provider() else {
            return self.inner.execute(request);
        };
        if request.method != reqwest::Method::GET
            || request.url.strip_prefix(endpoint.as_str()) != Some("/v1/models")
        {
            return self.inner.execute(request);
        }
        let explicit = request
            .headers
            .get("authorization")
            .and_then(|value| value.strip_prefix("Bearer "))
            .map(str::to_owned);
        let supplied = explicit.is_some();
        if let Some(secret) = explicit.or(key) {
            if !supplied {
                request
                    .headers
                    .insert("authorization".into(), format!("Bearer {secret}"));
            }
            return self
                .inner
                .execute(request)
                .map_err(|error| error.replace(&secret, "[REDACTED]"))
                .map(|response| redact_catalog_response(response, &secret));
        }
        if request.headers.contains_key("authorization") {
            return self.inner.execute(request);
        }
        // A stored credential shares the client's anonymous cache key with
        // unauthenticated reads. Never reuse an ETag across those identities.
        request.headers.remove("if-none-match");
        let Some(mut store) = crate::agent::credential::tokens::Store::open_existing()? else {
            return self.inner.execute(request);
        };
        let mut tried = Vec::new();
        let mut last_refusal = None;
        loop {
            let Some(selected) = store.next(&endpoint, &tried)? else {
                return match last_refusal {
                    Some(response) => Ok(response),
                    None => self.inner.execute(request),
                };
            };
            let mut attempt = request.clone();
            attempt.headers.insert(
                "authorization".into(),
                format!("Bearer {}", selected.secret),
            );
            let response = self
                .inner
                .execute(attempt)
                .map_err(|error| error.replace(&selected.secret, "[REDACTED]"))?;
            let response = redact_catalog_response(response, &selected.secret);
            if !credential_rejection_status(response.status) {
                return Ok(response);
            }
            store.failure(
                &selected.id,
                &selected.secret,
                "GET model catalog",
                &format!(
                    "HTTP {}: {}",
                    response.status,
                    String::from_utf8_lossy(&response.body)
                ),
            )?;
            tried.push(selected.id);
            last_refusal = Some(response);
        }
    }
}

static DIRECT_CATALOG_TRANSPORT: LazyLock<Arc<dyn transport::ControlPlaneTransport>> =
    LazyLock::new(|| {
        Arc::new(DirectCatalogTransport {
            inner: transport::ReqwestTransport::production(),
        })
    });

/// Resolve picker and slash catalogs through the run loop's selected route.
/// Direct catalog requests use the provider-aware transport's token rotation.
pub(crate) fn model_client() -> Result<brama::BramaClient, String> {
    let (endpoint, bearer) = match crate::agent::credential::direct_provider() {
        Some((endpoint, key)) => (Some(endpoint), key),
        None => {
            if let Some(said) = crate::agent::credential::direct_refusal() {
                return Err(said.to_owned());
            }
            let bearer = match std::env::var("BRAMA_TOKEN") {
                Ok(value) => Some(value),
                Err(std::env::VarError::NotPresent) => None,
                Err(std::env::VarError::NotUnicode(_)) => {
                    return Err("BRAMA_TOKEN cannot be read: value is not Unicode".into())
                }
            };
            (crate::agent::credential::brama_url(), bearer)
        }
    };
    Ok(brama::BramaClient::configured(endpoint, bearer))
}

pub fn model_catalog(
    cwd: &Path,
    client: &brama::BramaClient,
    force: bool,
) -> Result<brama::ModelCatalog, brama::BramaError> {
    let mut catalog = client.catalog(force)?;
    let mut entries = catalog
        .models
        .into_iter()
        .map(|entry| (entry.id.clone(), entry))
        .collect::<BTreeMap<_, _>>();
    for entry in
        crate::hooks::extension_model_entries(cwd).map_err(brama::BramaError::Configuration)?
    {
        entries.insert(entry.id.clone(), entry);
    }
    catalog.models = entries.into_values().collect();
    Ok(catalog)
}

fn credential_rejection_status(status: u16) -> bool {
    reqwest::StatusCode::from_u16(status).is_ok_and(|status| {
        matches!(
            status,
            reqwest::StatusCode::UNAUTHORIZED
                | reqwest::StatusCode::FORBIDDEN
                | reqwest::StatusCode::PAYMENT_REQUIRED
                | reqwest::StatusCode::TOO_MANY_REQUESTS
        )
    })
}

pub fn providers(
    cwd: &Path,
    client: &weles::WelesClient,
) -> Result<Vec<weles::Provider>, weles::WelesError> {
    let mut entries = client
        .providers()?
        .into_iter()
        .map(|entry| (entry.id.clone(), entry))
        .collect::<BTreeMap<_, _>>();
    for entry in
        crate::hooks::extension_provider_entries(cwd).map_err(weles::WelesError::Configuration)?
    {
        entries.insert(entry.id.clone(), entry);
    }
    Ok(entries.into_values().collect())
}
