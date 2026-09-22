//! Making one request to the platform billing service, and reading the answer
//! within declared bounds.
//!
//! Split out of `control_plane/services/weles.rs`, which had grown past the
//! module line cap.

use super::super::contract::RequestMeta;
use super::super::transport::{SecretRef, TransportRequest};
use super::{WelesClient, WelesError, API_VERSION, MAX_RESPONSE_BYTES};
use crate::control_plane::services::weles::contract::guards::reject_forbidden_payment_fields;
use serde::de::DeserializeOwned;
use serde_json::Value;
use std::collections::BTreeMap;

impl WelesClient {
    pub(super) fn request(
        &self,
        method: reqwest::Method,
        path: &str,
        body: Option<&Value>,
    ) -> Result<Value, WelesError> {
        let sequence = self
            .correlation
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
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
        if let Some(token) = self.authorization.as_ref().and_then(SecretRef::resolve) {
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
                max_response_bytes: MAX_RESPONSE_BYTES,
            })
            .map_err(WelesError::Transport)?;
        super::contract::negotiate_response(&response.headers).map_err(|error| {
            WelesError::InvalidResponse(format!("schema negotiation failed: {error:?}"))
        })?;
        if response.status == 429 {
            let retry_after_ms = response
                .headers
                .get("retry-after")
                .and_then(|value| value.parse::<u64>().ok())
                .map(|seconds| seconds.saturating_mul(1000));
            return Err(WelesError::RateLimited { retry_after_ms });
        }
        if !(200..300).contains(&response.status) {
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
        if super::contract::negotiate(meta.schema_min, meta.schema_max).ok() != Some(2) {
            return Err(WelesError::InvalidRequest(
                "Weles v2 requires schema version 2",
            ));
        }
        if meta.correlation_id.is_empty() {
            return Err(WelesError::InvalidRequest("correlation id is required"));
        }
        if financial_mutation && meta.idempotency_key.as_deref().is_none_or(str::is_empty) {
            return Err(WelesError::InvalidRequest(
                "financial mutation requires caller idempotency key",
            ));
        }
        if let Some(value) = body {
            reject_forbidden_payment_fields(value)?;
        }
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
                max_response_bytes: MAX_RESPONSE_BYTES,
            })
            .map_err(WelesError::Transport)?;
        super::contract::negotiate_response(&response.headers)
            .and_then(|version| {
                (version == 2).then_some(()).ok_or(
                    super::super::contract::ContractError::SchemaSkew {
                        service_min: version,
                        service_max: version,
                    },
                )
            })
            .map_err(|_| WelesError::InvalidResponse("schema negotiation failed".into()))?;
        if response.status == 429 {
            let retry_after_ms = response
                .headers
                .get("retry-after")
                .and_then(|value| value.parse::<u64>().ok())
                .map(|seconds| seconds.saturating_mul(1_000));
            return Err(WelesError::RateLimited { retry_after_ms });
        }
        if !(200..300).contains(&response.status) {
            return Err(WelesError::Http {
                status: response.status,
                message: "request failed; response body suppressed".into(),
            });
        }
        serde_json::from_slice(&response.body).map_err(|_| {
            WelesError::InvalidResponse(
                "response did not match the bounded Weles v2 contract".into(),
            )
        })
    }
}
