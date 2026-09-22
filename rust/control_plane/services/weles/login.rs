//! The login, refresh and logout conversations.
//!
//! Split out of `control_plane/services/weles.rs`, which had grown past the
//! module line cap.

use super::{
    Account, InteractionBridge, OperationEvent, OperationV1, Provider, WelesClient, WelesError,
    MAX_ACCOUNTS, MAX_POLL_EVENTS, MAX_PROVIDERS,
};
use crate::control_plane::brama::BramaClient;
use crate::control_plane::now_ms;
use serde_json::{json, Value};

impl WelesClient {
    pub fn providers(&self) -> Result<Vec<Provider>, WelesError> {
        let value = self.request(reqwest::Method::GET, "/providers", None)?;
        let providers: Vec<Provider> =
            serde_json::from_value(value.get("providers").cloned().unwrap_or(value))
                .map_err(|e| WelesError::InvalidResponse(e.to_string()))?;
        if providers.len() > MAX_PROVIDERS {
            return Err(WelesError::InvalidResponse(format!(
                "provider count exceeds {MAX_PROVIDERS}"
            )));
        }
        Ok(providers)
    }

    pub fn accounts(&self, provider: Option<&str>) -> Result<Vec<Account>, WelesError> {
        let path = provider
            .map(|id| {
                format!(
                    "/accounts?provider={}",
                    url::form_urlencoded::byte_serialize(id.as_bytes()).collect::<String>()
                )
            })
            .unwrap_or_else(|| "/accounts".into());
        let value = self.request(reqwest::Method::GET, &path, None)?;
        let accounts: Vec<Account> =
            serde_json::from_value(value.get("accounts").cloned().unwrap_or(value))
                .map_err(|e| WelesError::InvalidResponse(e.to_string()))?;
        if accounts.len() > MAX_ACCOUNTS {
            return Err(WelesError::InvalidResponse(format!(
                "account count exceeds {MAX_ACCOUNTS}"
            )));
        }
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
            return Err(WelesError::UnavailableProvider {
                provider: provider.id.clone(),
                reason: provider
                    .unavailable_reason
                    .clone()
                    .unwrap_or_else(|| "provider disabled".into()),
            });
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
        for _ in 0..MAX_POLL_EVENTS {
            if cancelled() {
                let _ = self.request(
                    reqwest::Method::POST,
                    &format!("/operations/{operation_id}/cancel"),
                    Some(&json!({})),
                );
                return Err(WelesError::Cancelled);
            }
            let suffix = cursor
                .as_ref()
                .map(|value| {
                    format!(
                        "?cursor={}",
                        url::form_urlencoded::byte_serialize(value.as_bytes()).collect::<String>()
                    )
                })
                .unwrap_or_default();
            let value = self.request(
                reqwest::Method::GET,
                &format!("/operations/{operation_id}{suffix}"),
                None,
            )?;
            let page: OperationV1 = serde_json::from_value(value)
                .map_err(|e| WelesError::InvalidResponse(e.to_string()))?;
            if page.expires_at_ms.is_some_and(|expiry| now_ms() >= expiry) {
                return Err(WelesError::ExpiredOperation);
            }
            cursor = page.cursor;
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
            match page.state.as_str() {
                "completed" => {
                    BramaClient::invalidate_all();
                    return Ok(None);
                }
                "failed" => {
                    return Err(WelesError::Operation {
                        code: "failed".into(),
                        message: "operation failed without an error event".into(),
                    })
                }
                "cancelled" => return Err(WelesError::Cancelled),
                _ => std::thread::sleep(self.poll_interval),
            }
        }
        Err(WelesError::PollLimit)
    }
}
