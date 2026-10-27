//! The login, refresh and logout conversations.
//!
//! Split out of `control_plane/services/weles.rs`, which had grown past the
//! module line cap.

use super::{
    Account, InteractionBridge, OperationEvent, OperationState, OperationV1, Provider, WelesClient,
    WelesError, HOLD_QUERY,
};
use crate::control_plane::brama::BramaClient;
use serde_json::{json, Value};

/// The list `key` names in a Weles answer: the answer itself when it is a
/// bare array, the array under `key` when it is an object carrying one; any
/// other shape is refused naming what was answered.
fn listed(value: Value, key: &str, path: &str) -> Result<Value, WelesError> {
    match value {
        Value::Array(_) => Ok(value),
        Value::Object(mut object) => object.remove(key).ok_or_else(|| {
            WelesError::InvalidResponse(format!("the answer to {path} carries no {key} list: {}", Value::Object(object)))
        }),
        other => Err(WelesError::InvalidResponse(format!(
            "the answer to {path} is neither a list nor an object holding {key}: {other}"
        ))),
    }
}

impl WelesClient {
    pub fn providers(&self) -> Result<Vec<Provider>, WelesError> {
        let value = self.request(reqwest::Method::GET, "/providers", None)?;
        let providers: Vec<Provider> = serde_json::from_value(listed(value, "providers", "/providers")?)
            .map_err(|e| WelesError::InvalidResponse(e.to_string()))?;
        Ok(providers)
    }

    pub fn accounts(&self, provider: Option<&str>) -> Result<Vec<Account>, WelesError> {
        let path = match provider {
            Some(id) => format!(
                "/accounts?provider={}",
                url::form_urlencoded::byte_serialize(id.as_bytes()).collect::<String>()
            ),
            None => "/accounts".to_string(),
        };
        let value = self.request(reqwest::Method::GET, &path, None)?;
        let accounts: Vec<Account> = serde_json::from_value(listed(value, "accounts", &path)?)
            .map_err(|e| WelesError::InvalidResponse(e.to_string()))?;
        Ok(accounts)
    }

    pub fn login(
        &self,
        provider: &str,
        consumer: &str,
        bridge: &dyn InteractionBridge,
        cancelled: &dyn Fn() -> bool,
    ) -> Result<Option<Account>, WelesError> {
        let advertised = self
            .providers()?
            .into_iter()
            .find(|entry| entry.id == provider)
            .ok_or_else(|| WelesError::UnknownProvider(provider.into()))?;
        self.login_provider(&advertised, consumer, bridge, cancelled)
    }

    pub fn login_provider(
        &self,
        provider: &Provider,
        consumer: &str,
        bridge: &dyn InteractionBridge,
        cancelled: &dyn Fn() -> bool,
    ) -> Result<Option<Account>, WelesError> {
        if !provider.available {
            let reason = match &provider.unavailable_reason {
                Some(reason) => reason.clone(),
                None => "Weles marks it unavailable and names no reason".to_string(),
            };
            return Err(WelesError::UnavailableProvider { provider: provider.id.clone(), reason });
        }
        self.run_operation(
            "/auth/login",
            json!({"provider": provider.id, "consumer": consumer}),
            bridge,
            cancelled,
        )
    }

    pub fn refresh(
        &self,
        account: &str,
        bridge: &dyn InteractionBridge,
        cancelled: &dyn Fn() -> bool,
    ) -> Result<Option<Account>, WelesError> {
        self.run_operation(
            "/auth/refresh",
            json!({"accountId": account}),
            bridge,
            cancelled,
        )
    }

    pub fn refresh_due(
        &self,
        bridge: &dyn InteractionBridge,
        cancelled: &dyn Fn() -> bool,
    ) -> Result<Vec<Account>, WelesError> {
        let accounts = self.accounts(None)?;
        let mut refreshed = Vec::new();
        for account in accounts
            .into_iter()
            .filter(|account| account.refresh_required || account.status == "expiring")
        {
            if let Some(account) = self.refresh(&account.id, bridge, cancelled)? {
                refreshed.push(account);
            }
        }
        Ok(refreshed)
    }

    pub fn logout(
        &self,
        account: &str,
        bridge: &dyn InteractionBridge,
        cancelled: &dyn Fn() -> bool,
    ) -> Result<Option<Account>, WelesError> {
        self.run_operation(
            "/auth/logout",
            json!({"accountId": account}),
            bridge,
            cancelled,
        )
    }

    fn run_operation(
        &self,
        path: &str,
        body: Value,
        bridge: &dyn InteractionBridge,
        cancelled: &dyn Fn() -> bool,
    ) -> Result<Option<Account>, WelesError> {
        let started = self.request(reqwest::Method::POST, path, Some(&body))?;
        let operation_id = started
            .get("operationId")
            .or_else(|| started.get("id"))
            .and_then(Value::as_str)
            .filter(|v| !v.is_empty())
            .ok_or_else(|| WelesError::InvalidResponse("operation id is missing".into()))?
            .to_string();
        let mut cursor: Option<String> = None;
        // One holding read per event: the deployment answers when the
        // operation has moved past the cursor or has ended. A page that
        // carries no new event and no end was not held, and the client
        // refuses rather than reading again on a timer.
        loop {
            if cancelled() {
                // A cancel Weles did not take leaves the sign-in running on
                // the deployment; that is the answer, never 'cancelled'.
                if let Err(error) = self.request(
                    reqwest::Method::POST,
                    &format!("/operations/{operation_id}/cancel"),
                    Some(&json!({})),
                ) {
                    return Err(WelesError::CancelRefused {
                        operation: operation_id.to_string(),
                        cause: error.to_string(),
                    });
                }
                return Err(WelesError::Cancelled);
            }
            let query = match cursor.as_ref() {
                Some(value) => format!(
                    "?{HOLD_QUERY}&cursor={}",
                    url::form_urlencoded::byte_serialize(value.as_bytes()).collect::<String>()
                ),
                None => format!("?{HOLD_QUERY}"),
            };
            let value = self.request(
                reqwest::Method::GET,
                &format!("/operations/{operation_id}{query}"),
                None,
            )?;
            let page: OperationV1 = serde_json::from_value(value)
                .map_err(|e| WelesError::InvalidResponse(e.to_string()))?;
            page.refuse_expired()?;
            cursor = page.cursor;
            let moved = !page.events.is_empty();
            for event in page.events {
                bridge.event(&event);
                match event {
                    OperationEvent::Elicit {
                        field,
                        prompt,
                        secret,
                        options,
                    } => {
                        let answer = bridge
                            .elicit(&prompt, &options, secret)
                            .map_err(WelesError::Interaction)?;
                        self.request(
                            reqwest::Method::POST,
                            &format!("/operations/{operation_id}/input"),
                            Some(&json!({"field": field, "value": answer})),
                        )?;
                    }
                    OperationEvent::Completed { account } => {
                        BramaClient::invalidate_all();
                        return Ok(account);
                    }
                    OperationEvent::Failed { code, message } => {
                        return Err(WelesError::Operation { code, message })
                    }
                    _ => {}
                }
            }
            match page.state {
                OperationState::Completed => {
                    BramaClient::invalidate_all();
                    return Ok(None);
                }
                OperationState::Failed => {
                    return Err(WelesError::Operation {
                        code: page.state.to_string(),
                        message: "operation failed without an error event".into(),
                    })
                }
                OperationState::Cancelled => return Err(WelesError::Cancelled),
                _ if moved => {}
                _ => return Err(WelesError::NoHoldingRead),
            }
        }
    }
}
