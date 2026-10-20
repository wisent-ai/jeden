//! Making one request to the platform billing service and reading its
//! answer whole.
//!
//! Split out of `control_plane/services/weles.rs`, which had grown past the
//! module line cap.

use super::super::contract::RequestMeta;
use super::super::transport::{SecretRef, TransportRequest};
use super::{WelesClient, WelesError, API_VERSION};
use serde::de::DeserializeOwned;
use serde_json::Value;
use std::collections::BTreeMap;

fn retry_after_ms(headers: &BTreeMap<String, String>) -> Result<Option<u64>, WelesError> {
    let Some(value) = headers.get("retry-after") else {
        return Ok(None);
    };
    let seconds = value.parse::<u64>().map_err(|error| {
        WelesError::InvalidResponse(format!(
            "Weles Retry-After header is not whole seconds ({value}): {error}"
        ))
    })?;
    Ok(Some(
        std::time::Duration::from_secs(seconds)
            .as_millis()
            .min(u128::from(u64::MAX)) as u64,
    ))
}

impl WelesClient {
    pub(super) fn request(
        &self,
        method: reqwest::Method,
        path: &str,
        body: Option<&Value>,
    ) -> Result<Value, WelesError> {
        let sequence = crate::tally::draw(&self.correlation);
        let meta = if method == reqwest::Method::GET {
            RequestMeta::read(format!("weles-{sequence}"))
        } else {
            RequestMeta::mutation(format!("weles-{sequence}"), format!("weles-{sequence}"))
        };
        self.request_with_meta(method, path, body, &meta)
    }

    pub(super) fn request_with_meta(
        &self,
        method: reqwest::Method,
        path: &str,
        body: Option<&Value>,
        meta: &RequestMeta,
    ) -> Result<Value, WelesError> {
        super::contract::negotiate(meta.schema_min, meta.schema_max).map_err(|error| {
            WelesError::InvalidResponse(format!("schema negotiation failed: {error:?}"))
        })?;
        if method != reqwest::Method::GET
            && meta.idempotency_key.as_deref().is_none_or(str::is_empty)
        {
            return Err(WelesError::InvalidResponse(
                "mutation requires idempotency key".into(),
            ));
        }
        let mut headers = BTreeMap::new();
        headers.insert("x-jeden-schema-min".into(), meta.schema_min.to_string());
        headers.insert("x-jeden-schema-max".into(), meta.schema_max.to_string());
        headers.insert("x-correlation-id".into(), meta.correlation_id.clone());
        if let Some(key) = &meta.idempotency_key {
            headers.insert("idempotency-key".into(), key.clone());
        }
        if let Some(token) = SecretRef::resolve_optional(self.authorization.as_ref())
            .map_err(WelesError::Configuration)?
        {
            headers.insert("authorization".into(), format!("Bearer {token}"));
        }
        let body = body
            .map(serde_json::to_vec)
            .transpose()
            .map_err(|error| WelesError::InvalidResponse(error.to_string()))?;
        let response = self
            .transport
            .execute(TransportRequest {
                method,
                url: format!("{}/{API_VERSION}{path}", self.endpoint()?),
                headers,
                body,
            })
            .map_err(WelesError::Transport)?;
        super::contract::negotiate_response(&response.headers).map_err(|error| {
            WelesError::InvalidResponse(format!("schema negotiation failed: {error:?}"))
        })?;
        if response.status == reqwest::StatusCode::TOO_MANY_REQUESTS.as_u16() {
            return Err(WelesError::RateLimited {
                retry_after_ms: retry_after_ms(&response.headers)?,
            });
        }
        if !reqwest::StatusCode::from_u16(response.status).is_ok_and(|status| status.is_success()) {
            return Err(WelesError::Http {
                status: response.status,
                message: "request failed; response body suppressed".into(),
            });
        }
        if response.body.is_empty() {
            return Ok(Value::Null);
        }
        serde_json::from_slice(&response.body)
            .map_err(|e| WelesError::InvalidResponse(e.to_string()))
    }

    pub(super) fn request_v2<T: DeserializeOwned>(
        &self,
        method: reqwest::Method,
        path: &str,
        body: Option<&Value>,
        meta: &RequestMeta,
        financial_mutation: bool,
    ) -> Result<T, WelesError> {
        let billing_schema = RequestMeta::read_v2("").schema_min;
        let negotiated =
            super::contract::negotiate(meta.schema_min, meta.schema_max).map_err(|error| {
                WelesError::Configuration(format!(
                    "Weles v2 requires its declared schema: {error:?}"
                ))
            })?;
        if negotiated != billing_schema {
            return Err(WelesError::Configuration(format!(
                "Weles v2 requires schema version {billing_schema}; negotiated {negotiated}"
            )));
        }
        if meta.correlation_id.is_empty() {
            return Err(WelesError::InvalidRequest("correlation id is required"));
        }
        if financial_mutation && meta.idempotency_key.as_deref().is_none_or(str::is_empty) {
            return Err(WelesError::InvalidRequest(
                "financial mutation requires caller idempotency key",
            ));
        }
        let mut headers = BTreeMap::new();
        headers.insert("x-jeden-schema-min".into(), meta.schema_min.to_string());
        headers.insert("x-jeden-schema-max".into(), meta.schema_max.to_string());
        headers.insert("x-correlation-id".into(), meta.correlation_id.clone());
        if let Some(key) = &meta.idempotency_key {
            headers.insert("idempotency-key".into(), key.clone());
        }
        if let Some(token) = SecretRef::resolve_optional(self.authorization.as_ref())
            .map_err(WelesError::Configuration)?
        {
            headers.insert("authorization".into(), format!("Bearer {token}"));
        }
        let body = body
            .map(serde_json::to_vec)
            .transpose()
            .map_err(|_| WelesError::InvalidRequest("request encoding failed"))?;
        let response = self
            .transport
            .execute(TransportRequest {
                method,
                url: format!("{}/v2{path}", self.endpoint()?),
                headers,
                body,
            })
            .map_err(WelesError::Transport)?;
        super::contract::negotiate_response(&response.headers)
            .and_then(|version| {
                (version == billing_schema).then_some(()).ok_or(
                    super::super::contract::ContractError::SchemaSkew {
                        service_min: version,
                        service_max: version,
                    },
                )
            })
            .map_err(|error| {
                WelesError::InvalidResponse(format!(
                    "Weles v2 schema negotiation failed: {error:?}"
                ))
            })?;
        if response.status == reqwest::StatusCode::TOO_MANY_REQUESTS.as_u16() {
            return Err(WelesError::RateLimited {
                retry_after_ms: retry_after_ms(&response.headers)?,
            });
        }
        if !reqwest::StatusCode::from_u16(response.status).is_ok_and(|status| status.is_success()) {
            return Err(WelesError::Http {
                status: response.status,
                message: "request failed; response body suppressed".into(),
            });
        }
        serde_json::from_slice(&response.body).map_err(|error| {
            WelesError::InvalidResponse(format!(
                "response did not match the bounded Weles v2 contract: {error}"
            ))
        })
    }
}
