//! The first version of the billing contract, kept because deployments still
//! answer it.
//!
//! Split out of `control_plane/services/weles.rs`, which had grown past the
//! module line cap.

use crate::control_plane::{now_ms, ServiceHealth};
use super::super::{Account, InteractionBridge, OperationV1, Provider, WelesClient, WelesError};
use serde_json::Value;

impl crate::control_plane::contract::WelesApiV1 for WelesClient {
    fn health(&self) -> ServiceHealth {
        WelesClient::health(self)
    }

    fn readiness(&self) -> Result<super::contract::Readiness, WelesError> {
        super::contract::negotiate(1, 1).map_err(|error| {
            WelesError::InvalidResponse(format!("schema negotiation failed: {error:?}"))
        })?;
        let providers = self.providers()?;
        Ok(super::contract::Readiness {
            ready: true,
            schema_min: 1,
            schema_max: 1,
            max_payload_bytes: MAX_RESPONSE_BYTES,
            detail: format!("{} providers advertised", providers.len()),
        })
    }

    fn providers(&self) -> Result<Vec<Provider>, WelesError> {
        WelesClient::providers(self)
    }
    fn accounts(&self, provider: Option<&str>) -> Result<Vec<Account>, WelesError> {
        WelesClient::accounts(self, provider)
    }

    fn begin_login(
        &self,
        provider: &str,
        consumer: &str,
        meta: &RequestMeta,
    ) -> Result<OperationV1, WelesError> {
        let value = self.request_with_meta(
            reqwest::Method::POST,
            "/auth/login",
            Some(&json!({"provider": provider, "consumer": consumer})),
            meta,
        )?;
        operation_from_start(value)
    }

    fn poll_operation(
        &self,
        operation_id: &str,
        cursor: Option<&str>,
        meta: &RequestMeta,
    ) -> Result<OperationV1, WelesError> {
        let suffix = cursor
            .map(|value| {
                format!(
                    "?cursor={}",
                    url::form_urlencoded::byte_serialize(value.as_bytes()).collect::<String>()
                )
            })
            .unwrap_or_default();
        let value = self.request_with_meta(
            reqwest::Method::GET,
            &format!("/operations/{operation_id}{suffix}"),
            None,
            meta,
        )?;
        let operation: OperationV1 = serde_json::from_value(value)
            .map_err(|error| WelesError::InvalidResponse(error.to_string()))?;
        if operation.id != operation_id {
            return Err(WelesError::InvalidResponse("operation id mismatch".into()));
        }
        if operation
            .expires_at_ms
            .is_some_and(|expiry| now_ms() >= expiry)
        {
            return Err(WelesError::ExpiredOperation);
        }
        if operation.state == "completed" {
            BramaClient::invalidate_all();
        }
        if operation.state == "cancelled" {
            return Err(WelesError::Cancelled);
        }
        Ok(operation)
    }

    fn submit_input(
        &self,
        operation_id: &str,
        field: &str,
        value: &str,
        meta: &RequestMeta,
    ) -> Result<(), WelesError> {
        self.request_with_meta(
            reqwest::Method::POST,
            &format!("/operations/{operation_id}/input"),
            Some(&json!({"field": field, "value": value})),
            meta,
        )
        .map(|_| ())
    }

    fn cancel_operation(&self, operation_id: &str, meta: &RequestMeta) -> Result<(), WelesError> {
        match self.request_with_meta(
            reqwest::Method::POST,
            &format!("/operations/{operation_id}/cancel"),
            Some(&json!({})),
            meta,
        ) {
            Ok(_) | Err(WelesError::Http { status: 409, .. }) => Ok(()),
            Err(error) => Err(error),
        }
    }

    fn refresh(&self, account: &str, meta: &RequestMeta) -> Result<OperationV1, WelesError> {
        let value = self.request_with_meta(
            reqwest::Method::POST,
            "/auth/refresh",
            Some(&json!({"accountId": account})),
            meta,
        )?;
        operation_from_start(value)
    }

    fn logout(&self, account: &str, meta: &RequestMeta) -> Result<OperationV1, WelesError> {
        let value = self.request_with_meta(
            reqwest::Method::POST,
            "/auth/logout",
            Some(&json!({"accountId": account})),
            meta,
        )?;
        operation_from_start(value)
    }
}

pub(super) fn operation_from_start(value: Value) -> Result<OperationV1, WelesError> {
    if value.get("operationId").is_some() && value.get("id").is_none() {
        let id = value
            .get("operationId")
            .and_then(Value::as_str)
            .filter(|id| !id.is_empty())
            .ok_or_else(|| WelesError::InvalidResponse("operation id is missing".into()))?;
        return Ok(OperationV1 {
            id: id.into(),
            state: "pending".into(),
            cursor: None,
            events: Vec::new(),
            expires_at_ms: value.get("expiresAtMs").and_then(Value::as_u64),
        });
    }
    let operation: OperationV1 = serde_json::from_value(value)
        .map_err(|error| WelesError::InvalidResponse(error.to_string()))?;
    if operation.id.trim().is_empty() {
        return Err(WelesError::InvalidResponse(
            "operation id is missing".into(),
        ));
    }
    if operation
        .expires_at_ms
        .is_some_and(|expiry| now_ms() >= expiry)
    {
        return Err(WelesError::ExpiredOperation);
    }
    Ok(operation)
}
