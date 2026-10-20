//! Versioned Brama request negotiation and response handling.

use super::super::contract::RequestMeta;
use super::super::transport::{SecretRef, TransportRequest, TransportResponse};
use super::{BramaClient, BramaError};
use crate::control_plane::services::brama::auth::insert_caller_auth_headers;
use crate::control_plane::services::brama::API_VERSION;
use reqwest::StatusCode;
use std::collections::BTreeMap;
use std::time::Duration;

impl BramaClient {
    /// One versioned request, its answer read whole: a JSON reply, a
    /// generated image or a spoken text alike.
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
        if let Some(token) = SecretRef::resolve_optional(self.authorization.as_ref())
            .map_err(BramaError::Configuration)?
        {
            headers.insert("authorization".into(), format!("Bearer {token}"));
        }
        let signature_body = match body.as_deref() {
            Some(bytes) => bytes,
            None => &[],
        };
        insert_caller_auth_headers(&mut headers, signature_body);
        let response = self
            .transport
            .execute(TransportRequest {
                method,
                url: format!("{}/{API_VERSION}{path}", self.key()?),
                headers,
                body,
            })
            .map_err(BramaError::Transport)?;
        super::super::contract::negotiate_response(&response.headers).map_err(|error| {
            BramaError::InvalidResponse(format!("schema negotiation failed: {error:?}"))
        })?;
        if response.status == StatusCode::TOO_MANY_REQUESTS.as_u16() {
            let retry_after_ms = match response.headers.get("retry-after") {
                Some(value) => {
                    let seconds = value.parse::<u64>().map_err(|error| {
                        BramaError::InvalidResponse(format!(
                            "Brama Retry-After header is not whole seconds ({value}): {error}"
                        ))
                    })?;
                    Some(
                        Duration::from_secs(seconds)
                            .as_millis()
                            .min(u64::MAX as u128) as u64,
                    )
                }
                None => None,
            };
            return Err(BramaError::RateLimited {
                retry_after_ms,
                retryable: BramaError::stated_retryable(&response.body),
            });
        }
        if !StatusCode::from_u16(response.status).is_ok_and(|status| status.is_success()) {
            return Err(BramaError::http(response.status, path, &response.body));
        }
        Ok(response)
    }
}
