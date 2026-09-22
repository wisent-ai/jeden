use super::contract::RequestMeta;
use super::transport::{ControlPlaneTransport, ReqwestTransport, SecretRef, TransportRequest};
use super::{brama::BramaClient, now_ms, ServiceHealth};
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::Duration;

const API_VERSION: &str = "v1";
const PLATFORM_BILLING_URL_ENV: &str = "WISENT_PLATFORM_BILLING_URL";
const PLATFORM_BILLING_TOKEN_ENV: &str = "WISENT_PLATFORM_BILLING_TOKEN";
const LEGACY_BILLING_URL_ENV: &str = "WELES_URL";
const LEGACY_BILLING_TOKEN_ENV: &str = "WELES_TOKEN";
const MAX_POLL_EVENTS: usize = 256;
const MAX_RESPONSE_BYTES: u64 = 2 * 1024 * 1024;
const MAX_PROVIDERS: usize = 128;
const MAX_ACCOUNTS: usize = 512;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct Provider {
    pub id: String,
    pub display_name: String,
    #[serde(default)]
    pub login_methods: Vec<LoginMethod>,
    #[serde(default = "default_true")]
    pub available: bool,
    #[serde(default)]
    pub unavailable_reason: Option<String>,
}
fn default_true() -> bool {
    true
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum LoginMethod {
    DeviceCode,
    Paste,
    ApiKey,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct Account {
    pub id: String,
    pub provider: String,
    pub display_name: String,
    pub status: String,
    #[serde(default)]
    pub expires_at: Option<String>,
    #[serde(default)]
    pub refresh_required: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase", tag = "type")]
pub enum OperationEvent {
    Status {
        message: String,
    },
    DeviceCode {
        verification_uri: String,
        user_code: String,
        #[serde(default)]
        expires_in_seconds: Option<u64>,
    },
    Elicit {
        field: String,
        prompt: String,
        #[serde(default)]
        secret: bool,
        #[serde(default)]
        options: Vec<String>,
    },
    Completed {
        #[serde(default)]
        account: Option<Account>,
    },
    Failed {
        code: String,
        message: String,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct OperationV1 {
    pub id: String,
    pub state: String,
    #[serde(default)]
    pub cursor: Option<String>,
    #[serde(default)]
    pub events: Vec<OperationEvent>,
    #[serde(default)]
    pub expires_at_ms: Option<u64>,
}

pub trait InteractionBridge {
    fn elicit(&self, prompt: &str, options: &[String], secret: bool) -> Result<String, String>;
    fn event(&self, event: &OperationEvent);
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WelesError {
    Unconfigured,
    Transport(String),
    Http { status: u16, message: String },
    InvalidResponse(String),
    InvalidRequest(&'static str),
    UnknownProvider(String),
    UnavailableProvider { provider: String, reason: String },
    Cancelled,
    RateLimited { retry_after_ms: Option<u64> },
    ExpiredOperation,
    PollLimit,
    Operation { code: String, message: String },
    Interaction(String),
}
impl std::fmt::Display for WelesError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Unconfigured => f.write_str("Weles service endpoint is not configured"),
            Self::Transport(e) => write!(f, "Weles transport error: {e}"),
            Self::Http { status, message } => write!(f, "Weles returned HTTP {status}: {message}"),
            Self::InvalidResponse(e) => write!(f, "invalid Weles response: {e}"),
            Self::InvalidRequest(message) => write!(f, "invalid Weles request: {message}"),
            Self::UnknownProvider(id) => write!(f, "provider `{id}` is not advertised by Weles"),
            Self::UnavailableProvider { provider, reason } => {
                write!(f, "provider `{provider}` is unavailable: {reason}")
            }
            Self::Cancelled => f.write_str("Weles operation cancelled"),
            Self::RateLimited { retry_after_ms } => write!(
                f,
                "Weles rate limited the request; retry after {:?} ms",
                retry_after_ms
            ),
            Self::ExpiredOperation => f.write_str("Weles operation expired"),
            Self::PollLimit => f.write_str("Weles operation exceeded its event limit"),
            Self::Operation { code, message } => {
                write!(f, "Weles operation failed ({code}): {message}")
            }
            Self::Interaction(e) => write!(f, "Weles interaction failed: {e}"),
        }
    }
}

#[derive(Clone)]
pub struct WelesClient {
    endpoint: Option<String>,
    authorization: Option<SecretRef>,
    transport: Arc<dyn ControlPlaneTransport>,
    poll_interval: Duration,
    correlation: Arc<std::sync::atomic::AtomicU64>,
}

pub(crate) fn platform_billing_configured() -> bool {
    [PLATFORM_BILLING_URL_ENV, LEGACY_BILLING_URL_ENV]
        .iter()
        .any(|name| {
            std::env::var(name)
                .ok()
                .is_some_and(|value| !value.trim().is_empty())
        })
}

fn platform_billing_endpoint() -> Option<String> {
    std::env::var(PLATFORM_BILLING_URL_ENV)
        .or_else(|_| std::env::var(LEGACY_BILLING_URL_ENV))
        .ok()
}

fn platform_billing_token() -> SecretRef {
    if std::env::var_os(PLATFORM_BILLING_TOKEN_ENV).is_some() {
        SecretRef::environment(PLATFORM_BILLING_TOKEN_ENV)
    } else {
        SecretRef::environment(LEGACY_BILLING_TOKEN_ENV)
    }
}

impl WelesClient {
    pub fn from_env() -> Self {
        Self::with_secret_ref(
            platform_billing_endpoint(),
            Some(platform_billing_token()),
            Duration::from_millis(500),
            ReqwestTransport::production(),
        )
    }

    pub fn new(endpoint: Option<String>, bearer: Option<String>, poll_interval: Duration) -> Self {
        Self::with_transport(
            endpoint,
            bearer,
            poll_interval,
            ReqwestTransport::production(),
        )
    }

    pub fn with_transport(
        endpoint: Option<String>,
        bearer: Option<String>,
        poll_interval: Duration,
        transport: Arc<dyn ControlPlaneTransport>,
    ) -> Self {
        Self::with_secret_ref(
            endpoint,
            bearer.map(SecretRef::inline),
            poll_interval,
            transport,
        )
    }

    pub fn with_secret_ref(
        endpoint: Option<String>,
        authorization: Option<SecretRef>,
        poll_interval: Duration,
        transport: Arc<dyn ControlPlaneTransport>,
    ) -> Self {
        let endpoint = endpoint
            .map(|v| v.trim_end_matches('/').to_string())
            .filter(|v| !v.is_empty());
        Self {
            endpoint,
            authorization,
            transport,
            poll_interval,
            correlation: Arc::new(std::sync::atomic::AtomicU64::new(1)),
        }
    }

    fn endpoint(&self) -> Result<&str, WelesError> {
        self.endpoint.as_deref().ok_or(WelesError::Unconfigured)
    }

    pub fn health(&self) -> ServiceHealth {
        let available = self.endpoint.is_some();
        ServiceHealth {
            service: "platform-billing".into(),
            version: API_VERSION.into(),
            available,
            endpoint: self.endpoint.clone(),
            detail: if available {
                "configured; provider state is resolved on demand".into()
            } else {
                "WISENT_PLATFORM_BILLING_URL is not configured".into()
            },
            checked_at_ms: now_ms(),
        }
    }

    fn request(
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

    fn request_with_meta(
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

    fn request_v2<T: DeserializeOwned>(
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
                (version == 2)
                    .then_some(())
                    .ok_or(super::contract::ContractError::SchemaSkew {
                        service_min: version,
                        service_max: version,
                    })
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

    fn validate_hosted_setup(
        &self,
        setup: &super::billing::HostedPaymentSetup,
    ) -> Result<(), WelesError> {
        let hosted = url::Url::parse(&setup.hosted_url)
            .map_err(|_| WelesError::InvalidResponse("hosted setup URL is invalid".into()))?;
        if hosted.scheme() != "https" || hosted.host_str().is_none() {
            return Err(WelesError::InvalidResponse(
                "hosted setup URL must use HTTPS".into(),
            ));
        }
        let hosted_origin = hosted.origin().ascii_serialization();
        let endpoint = url::Url::parse(self.endpoint()?)
            .map_err(|_| WelesError::InvalidResponse("Weles endpoint URL is invalid".into()))?;
        let endpoint_origin = endpoint.origin().ascii_serialization();
        let configured = std::env::var("WELES_HOSTED_ORIGINS").unwrap_or_default();
        if hosted_origin != endpoint_origin
            && !configured
                .split(',')
                .map(str::trim)
                .filter(|origin| !origin.is_empty())
                .any(|origin| origin == hosted_origin)
        {
            return Err(WelesError::InvalidResponse(
                "hosted setup URL origin is not allowlisted".into(),
            ));
        }
        Ok(())
    }

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

impl super::contract::WelesApiV1 for WelesClient {
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

fn operation_from_start(value: Value) -> Result<OperationV1, WelesError> {
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

fn encode_path_segment(value: &str) -> String {
    const HEX: &[u8; 16] = b"0123456789ABCDEF";
    let mut encoded = String::with_capacity(value.len());
    for byte in value.bytes() {
        if byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'.' | b'_' | b'~') {
            encoded.push(char::from(byte));
        } else {
            encoded.push('%');
            encoded.push(char::from(HEX[usize::from(byte >> 4)]));
            encoded.push(char::from(HEX[usize::from(byte & 0x0f)]));
        }
    }
    encoded
}

fn reject_forbidden_payment_fields(value: &Value) -> Result<(), WelesError> {
    match value {
        Value::Object(fields) => {
            for (key, child) in fields {
                let normalized = key
                    .chars()
                    .filter(|character| character.is_ascii_alphanumeric())
                    .flat_map(char::to_lowercase)
                    .collect::<String>();
                if matches!(
                    normalized.as_str(),
                    "pan"
                        | "cardnumber"
                        | "cvv"
                        | "cvc"
                        | "processortoken"
                        | "rawpaymentdetails"
                        | "billingaddress"
                        | "shippingaddress"
                        | "fulladdress"
                        | "cardholder"
                        | "expiry"
                        | "expiration"
                ) {
                    return Err(WelesError::InvalidRequest(
                        "raw payment fields are forbidden",
                    ));
                }
                reject_forbidden_payment_fields(child)?;
            }
        }
        Value::Array(items) => {
            for item in items {
                reject_forbidden_payment_fields(item)?;
            }
        }
        _ => {}
    }
    Ok(())
}

fn decode_v2<T: DeserializeOwned>(value: Value) -> Result<T, WelesError> {
    serde_json::from_value(value).map_err(|_| {
        WelesError::InvalidResponse("response did not match the bounded Weles v2 contract".into())
    })
}

fn validate_identifier(value: &str) -> Result<(), WelesError> {
    if value.is_empty()
        || value.len() > super::billing::MAX_BILLING_STRING_BYTES
        || value.chars().any(char::is_control)
    {
        return Err(WelesError::InvalidResponse(
            "billing identifier is invalid".into(),
        ));
    }
    Ok(())
}

impl super::contract::WelesApiV2 for WelesClient {
    fn billing_status(
        &self,
        account_id: &str,
        meta: &RequestMeta,
    ) -> Result<super::billing::AccountStatus, WelesError> {
        let status: super::billing::AccountStatus = self.request_v2(
            reqwest::Method::GET,
            &format!(
                "/accounts/{}/billing/status",
                encode_path_segment(account_id)
            ),
            None,
            meta,
            false,
        )?;
        validate_identifier(&status.account_id)?;
        validate_identifier(&status.provider_id)?;
        if status.account_id != account_id
            || status.capabilities.len() > super::billing::MAX_BILLING_ITEMS
        {
            return Err(WelesError::InvalidResponse(
                "billing status identity or capability count is invalid".into(),
            ));
        }
        Ok(status)
    }

    fn payment_methods(
        &self,
        account_id: &str,
        meta: &RequestMeta,
    ) -> Result<Vec<super::billing::PaymentMethodReference>, WelesError> {
        let value: Value = self.request_v2(
            reqwest::Method::GET,
            &format!(
                "/accounts/{}/payment-methods",
                encode_path_segment(account_id)
            ),
            None,
            meta,
            false,
        )?;
        let methods: Vec<super::billing::PaymentMethodReference> =
            decode_v2(value.get("paymentMethods").cloned().unwrap_or(value))?;
        if methods.len() > super::billing::MAX_BILLING_ITEMS {
            return Err(WelesError::InvalidResponse(
                "payment method count exceeds limit".into(),
            ));
        }
        Ok(methods)
    }

    fn begin_payment_method_setup(
        &self,
        request: &super::billing::PaymentMethodSetupRequest,
        meta: &RequestMeta,
    ) -> Result<super::billing::HostedPaymentSetup, WelesError> {
        validate_identifier(&request.account_id)
            .map_err(|_| WelesError::InvalidRequest("account id is invalid"))?;
        let return_url = url::Url::parse(&request.return_url)
            .map_err(|_| WelesError::InvalidRequest("return URL is invalid"))?;
        if return_url.scheme() != "https" {
            return Err(WelesError::InvalidRequest("return URL must use HTTPS"));
        }
        let body = serde_json::to_value(request)
            .map_err(|_| WelesError::InvalidRequest("setup request encoding failed"))?;
        let setup = self.request_v2(
            reqwest::Method::POST,
            "/payment-method-setups",
            Some(&body),
            meta,
            true,
        )?;
        self.validate_hosted_setup(&setup)?;
        Ok(setup)
    }

    fn revoke_payment_method(
        &self,
        account_id: &str,
        payment_method: &super::billing::PaymentMethodReference,
        meta: &RequestMeta,
    ) -> Result<super::billing::OperationResult, WelesError> {
        self.request_v2(
            reqwest::Method::POST,
            &format!(
                "/accounts/{}/payment-methods/{}/revoke",
                encode_path_segment(account_id),
                encode_path_segment(payment_method.as_str())
            ),
            Some(&json!({})),
            meta,
            true,
        )
    }

    fn purchase_policy(
        &self,
        account_id: &str,
        meta: &RequestMeta,
    ) -> Result<super::billing::PurchasePolicy, WelesError> {
        let policy = self.request_v2(
            reqwest::Method::GET,
            &format!(
                "/accounts/{}/purchase-policy",
                encode_path_segment(account_id)
            ),
            None,
            meta,
            false,
        )?;
        super::billing::validate_policy(&policy)
            .map_err(|message| WelesError::InvalidResponse(message.into()))?;
        Ok(policy)
    }

    fn set_purchase_policy(
        &self,
        account_id: &str,
        policy: &super::billing::PurchasePolicy,
        meta: &RequestMeta,
    ) -> Result<super::billing::PurchasePolicy, WelesError> {
        super::billing::validate_policy(policy).map_err(WelesError::InvalidRequest)?;
        let body = serde_json::to_value(policy)
            .map_err(|_| WelesError::InvalidRequest("policy encoding failed"))?;
        let applied = self.request_v2(
            reqwest::Method::PUT,
            &format!(
                "/accounts/{}/purchase-policy",
                encode_path_segment(account_id)
            ),
            Some(&body),
            meta,
            true,
        )?;
        super::billing::validate_policy(&applied)
            .map_err(|message| WelesError::InvalidResponse(message.into()))?;
        Ok(applied)
    }

    fn disable_purchase_policy(
        &self,
        account_id: &str,
        policy_revision: &str,
        meta: &RequestMeta,
    ) -> Result<super::billing::PurchasePolicy, WelesError> {
        validate_identifier(policy_revision)
            .map_err(|_| WelesError::InvalidRequest("policy revision is invalid"))?;
        let body = json!({"policyRevision": policy_revision});
        let policy = self.request_v2(
            reqwest::Method::POST,
            &format!(
                "/accounts/{}/purchase-policy/disable",
                encode_path_segment(account_id)
            ),
            Some(&body),
            meta,
            true,
        )?;
        super::billing::validate_policy(&policy)
            .map_err(|message| WelesError::InvalidResponse(message.into()))?;
        if policy.enabled || policy.auto_renew {
            return Err(WelesError::InvalidResponse(
                "disabled policy remains enabled".into(),
            ));
        }
        Ok(policy)
    }

    fn subscriptions(
        &self,
        account_id: &str,
        meta: &RequestMeta,
    ) -> Result<Vec<super::billing::SubscriptionV2>, WelesError> {
        let value: Value = self.request_v2(
            reqwest::Method::GET,
            &format!(
                "/accounts/{}/subscriptions",
                encode_path_segment(account_id)
            ),
            None,
            meta,
            false,
        )?;
        let subscriptions: Vec<super::billing::SubscriptionV2> =
            decode_v2(value.get("subscriptions").cloned().unwrap_or(value))?;
        if subscriptions.len() > super::billing::MAX_BILLING_ITEMS
            || subscriptions
                .iter()
                .any(|subscription| subscription.account_id != account_id)
        {
            return Err(WelesError::InvalidResponse(
                "subscription count or account identity is invalid".into(),
            ));
        }
        Ok(subscriptions)
    }

    fn quota(
        &self,
        subscription_id: &str,
        meta: &RequestMeta,
    ) -> Result<super::billing::QuotaSnapshot, WelesError> {
        let quota: super::billing::QuotaSnapshot = self.request_v2(
            reqwest::Method::GET,
            &format!(
                "/subscriptions/{}/quota",
                encode_path_segment(subscription_id)
            ),
            None,
            meta,
            false,
        )?;
        if quota.subscription_id != subscription_id
            || quota.buckets.len() > super::billing::MAX_BILLING_ITEMS
            || quota.buckets.iter().any(|bucket| {
                matches!(
                    (bucket.remaining, bucket.limit),
                    (Some(remaining), Some(limit)) if remaining > limit
                )
            })
        {
            return Err(WelesError::InvalidResponse(
                "quota identity, count, or remaining amount is invalid".into(),
            ));
        }
        Ok(quota)
    }

    fn quote(
        &self,
        request: &super::billing::QuoteRequest,
        meta: &RequestMeta,
    ) -> Result<super::billing::Quote, WelesError> {
        let body = serde_json::to_value(request)
            .map_err(|_| WelesError::InvalidRequest("quote request encoding failed"))?;
        let quote: super::billing::Quote = self.request_v2(
            reqwest::Method::POST,
            "/subscription-quotes",
            Some(&body),
            meta,
            false,
        )?;
        if quote.account_id != request.account_id
            || quote.provider_id != request.provider_id
            || quote.product_id != request.product_id
            || quote.currency != request.currency
        {
            return Err(WelesError::InvalidResponse(
                "quote identity does not match request".into(),
            ));
        }
        Ok(quote)
    }

    fn purchase(
        &self,
        request: &super::billing::PurchaseRequest,
        meta: &RequestMeta,
    ) -> Result<super::billing::OperationResult, WelesError> {
        let body = serde_json::to_value(request)
            .map_err(|_| WelesError::InvalidRequest("purchase request encoding failed"))?;
        self.request_v2(
            reqwest::Method::POST,
            "/subscriptions/purchase",
            Some(&body),
            meta,
            true,
        )
    }

    fn renew(
        &self,
        subscription_id: &str,
        request: &super::billing::RenewRequest,
        meta: &RequestMeta,
    ) -> Result<super::billing::OperationResult, WelesError> {
        let body = serde_json::to_value(request)
            .map_err(|_| WelesError::InvalidRequest("renew request encoding failed"))?;
        self.request_v2(
            reqwest::Method::POST,
            &format!(
                "/subscriptions/{}/renew",
                encode_path_segment(subscription_id)
            ),
            Some(&body),
            meta,
            true,
        )
    }

    fn cancel_subscription(
        &self,
        subscription_id: &str,
        meta: &RequestMeta,
    ) -> Result<super::billing::OperationResult, WelesError> {
        self.request_v2(
            reqwest::Method::POST,
            &format!(
                "/subscriptions/{}/cancel",
                encode_path_segment(subscription_id)
            ),
            Some(&json!({})),
            meta,
            true,
        )
    }
}
