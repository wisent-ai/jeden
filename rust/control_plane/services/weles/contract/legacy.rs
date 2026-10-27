//! The first version of the billing contract, kept because deployments still
//! answer it.
//!
//! Split out of `control_plane/services/weles.rs`, which had grown past the
//! module line cap.

use super::super::{Account, OperationState, OperationV1, Provider, WelesClient, WelesError};
use crate::control_plane::brama::BramaClient;
use crate::control_plane::contract::{self, RequestMeta};
use crate::control_plane::ServiceHealth;
use serde_json::json;
use serde_json::Value;

impl crate::control_plane::contract::WelesApiV1 for WelesClient {
    fn health(&self) -> ServiceHealth {
        WelesClient::health(self)
    }

    fn readiness(&self) -> Result<contract::Readiness, WelesError> {
        // This client speaks only the first Weles schema.
        let schema = crate::tally::FIRST as u32;
        contract::negotiate(schema, schema).map_err(|error| {
            WelesError::InvalidResponse(format!("schema negotiation failed: {error:?}"))
        })?;
        let providers = self.providers()?;
        Ok(contract::Readiness {
            ready: true,
            schema_min: schema,
            schema_max: schema,
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
        // A read without a cursor starts from the operation's first event.
        let suffix = match cursor {
            Some(value) => format!(
                "?cursor={}",
                url::form_urlencoded::byte_serialize(value.as_bytes()).collect::<String>()
            ),
            None => String::new(),
        };
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
        operation.refuse_expired()?;
        if operation.state == OperationState::Completed {
            BramaClient::invalidate_all();
        }
        if operation.state == OperationState::Cancelled {
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
        .map(|_acknowledgement| ())
    }

    fn cancel_operation(&self, operation_id: &str, meta: &RequestMeta) -> Result<(), WelesError> {
        match self.request_with_meta(
            reqwest::Method::POST,
            &format!("/operations/{operation_id}/cancel"),
            Some(&json!({})),
            meta,
        ) {
            // An operation that already ended answers a cancel with a conflict.
            Ok(_) => Ok(()),
            Err(WelesError::Http { status, .. })
                if status == reqwest::StatusCode::CONFLICT.as_u16() =>
            {
                Ok(())
            }
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
            state: OperationState::Pending,
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
    operation.refuse_expired()?;
    Ok(operation)
}
