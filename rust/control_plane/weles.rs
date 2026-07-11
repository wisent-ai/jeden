use super::{brama::BramaClient, now_ms, ServiceHealth};
use reqwest::blocking::Client;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::io::Read;
use std::time::Duration;

const API_VERSION: &str = "v1";
const MAX_POLL_EVENTS: usize = 256;
const MAX_RESPONSE_BYTES: u64 = 2 * 1024 * 1024;
const MAX_PROVIDERS: usize = 128;
const MAX_ACCOUNTS: usize = 512;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub(crate) struct Provider {
    pub id: String,
    pub display_name: String,
    #[serde(default)] pub login_methods: Vec<LoginMethod>,
    #[serde(default = "default_true")] pub available: bool,
    #[serde(default)] pub unavailable_reason: Option<String>,
}
fn default_true() -> bool { true }

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub(crate) enum LoginMethod { DeviceCode, Paste, ApiKey }

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub(crate) struct Account {
    pub id: String,
    pub provider: String,
    pub display_name: String,
    pub status: String,
    #[serde(default)] pub expires_at: Option<String>,
    #[serde(default)] pub refresh_required: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase", tag = "type")]
pub(crate) enum OperationEvent {
    Status { message: String },
    DeviceCode { verification_uri: String, user_code: String, #[serde(default)] expires_in_seconds: Option<u64> },
    Elicit { field: String, prompt: String, #[serde(default)] secret: bool, #[serde(default)] options: Vec<String> },
    Completed { #[serde(default)] account: Option<Account> },
    Failed { code: String, message: String },
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
struct OperationPage {
    pub id: String,
    pub state: String,
    #[serde(default)] pub cursor: Option<String>,
    #[serde(default)] pub events: Vec<OperationEvent>,
}

pub(crate) trait InteractionBridge {
    fn elicit(&self, prompt: &str, options: &[String], secret: bool) -> Result<String, String>;
    fn event(&self, event: &OperationEvent);
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum WelesError {
    Unconfigured,
    Transport(String),
    Http { status: u16, message: String },
    InvalidResponse(String),
    UnknownProvider(String),
    UnavailableProvider { provider: String, reason: String },
    Cancelled,
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
            Self::UnknownProvider(id) => write!(f, "provider `{id}` is not advertised by Weles"),
            Self::UnavailableProvider { provider, reason } => write!(f, "provider `{provider}` is unavailable: {reason}"),
            Self::Cancelled => f.write_str("Weles operation cancelled"),
            Self::PollLimit => f.write_str("Weles operation exceeded its event limit"),
            Self::Operation { code, message } => write!(f, "Weles operation failed ({code}): {message}"),
            Self::Interaction(e) => write!(f, "Weles interaction failed: {e}"),
        }
    }
}

#[derive(Clone)]
pub(crate) struct WelesClient {
    endpoint: Option<String>,
    bearer: Option<String>,
    http: Client,
    poll_interval: Duration,
}

impl WelesClient {
    pub(crate) fn from_env() -> Self {
        Self::new(std::env::var("WELES_URL").ok(), std::env::var("WELES_TOKEN").ok(), Duration::from_millis(500))
    }

    pub(crate) fn new(endpoint: Option<String>, bearer: Option<String>, poll_interval: Duration) -> Self {
        let endpoint = endpoint.map(|v| v.trim_end_matches('/').to_string()).filter(|v| !v.is_empty());
        let http = Client::builder().connect_timeout(Duration::from_secs(5)).timeout(Duration::from_secs(20)).build().unwrap_or_else(|_| Client::new());
        Self { endpoint, bearer: bearer.filter(|v| !v.trim().is_empty()), http, poll_interval }
    }

    fn endpoint(&self) -> Result<&str, WelesError> { self.endpoint.as_deref().ok_or(WelesError::Unconfigured) }

    pub(crate) fn health(&self) -> ServiceHealth {
        let available = self.endpoint.is_some();
        ServiceHealth { service: "weles".into(), version: API_VERSION.into(), available, endpoint: self.endpoint.clone(), detail: if available { "configured; provider state is resolved on demand".into() } else { "WELES_URL is not configured".into() }, checked_at_ms: now_ms() }
    }

    fn request(&self, method: reqwest::Method, path: &str, body: Option<&Value>) -> Result<Value, WelesError> {
        let mut request = self.http.request(method, format!("{}/{API_VERSION}{path}", self.endpoint()?));
        if let Some(token) = &self.bearer { request = request.bearer_auth(token); }
        if let Some(body) = body { request = request.json(body); }
        let response = request.send().map_err(|e| WelesError::Transport(e.to_string()))?;
        let status = response.status();
        if response.content_length().is_some_and(|length| length > MAX_RESPONSE_BYTES) { return Err(WelesError::InvalidResponse("response exceeds 2 MiB".into())); }
        let mut body = Vec::new();
        response.take(MAX_RESPONSE_BYTES + 1).read_to_end(&mut body).map_err(|e| WelesError::Transport(e.to_string()))?;
        if body.len() as u64 > MAX_RESPONSE_BYTES { return Err(WelesError::InvalidResponse("response exceeds 2 MiB".into())); }
        let text = String::from_utf8(body).map_err(|e| WelesError::InvalidResponse(e.to_string()))?;
        if !status.is_success() { return Err(WelesError::Http { status: status.as_u16(), message: text.chars().take(800).collect() }); }
        serde_json::from_str(&text).map_err(|e| WelesError::InvalidResponse(e.to_string()))
    }

    pub(crate) fn providers(&self) -> Result<Vec<Provider>, WelesError> {
        let value = self.request(reqwest::Method::GET, "/providers", None)?;
        let providers: Vec<Provider> = serde_json::from_value(value.get("providers").cloned().unwrap_or(value)).map_err(|e| WelesError::InvalidResponse(e.to_string()))?;
        if providers.len() > MAX_PROVIDERS { return Err(WelesError::InvalidResponse(format!("provider count exceeds {MAX_PROVIDERS}"))); }
        Ok(providers)
    }

    pub(crate) fn accounts(&self, provider: Option<&str>) -> Result<Vec<Account>, WelesError> {
        let path = provider.map(|id| format!("/accounts?provider={}", url::form_urlencoded::byte_serialize(id.as_bytes()).collect::<String>())).unwrap_or_else(|| "/accounts".into());
        let value = self.request(reqwest::Method::GET, &path, None)?;
        let accounts: Vec<Account> = serde_json::from_value(value.get("accounts").cloned().unwrap_or(value)).map_err(|e| WelesError::InvalidResponse(e.to_string()))?;
        if accounts.len() > MAX_ACCOUNTS { return Err(WelesError::InvalidResponse(format!("account count exceeds {MAX_ACCOUNTS}"))); }
        Ok(accounts)
    }

    pub(crate) fn login(&self, provider: &str, consumer: &str, bridge: &dyn InteractionBridge, cancelled: &dyn Fn() -> bool) -> Result<Option<Account>, WelesError> {
        let advertised = self.providers()?.into_iter().find(|entry| entry.id == provider).ok_or_else(|| WelesError::UnknownProvider(provider.into()))?;
        self.login_provider(&advertised, consumer, bridge, cancelled)
    }

    pub(crate) fn login_provider(&self, provider: &Provider, consumer: &str, bridge: &dyn InteractionBridge, cancelled: &dyn Fn() -> bool) -> Result<Option<Account>, WelesError> {
        if !provider.available { return Err(WelesError::UnavailableProvider { provider: provider.id.clone(), reason: provider.unavailable_reason.clone().unwrap_or_else(|| "provider disabled".into()) }); }
        self.run_operation("/auth/login", json!({"provider": provider.id, "consumer": consumer}), bridge, cancelled)
    }

    pub(crate) fn refresh(&self, account: &str, bridge: &dyn InteractionBridge, cancelled: &dyn Fn() -> bool) -> Result<Option<Account>, WelesError> {
        self.run_operation("/auth/refresh", json!({"accountId": account}), bridge, cancelled)
    }

    pub(crate) fn refresh_due(&self, bridge: &dyn InteractionBridge, cancelled: &dyn Fn() -> bool) -> Result<Vec<Account>, WelesError> {
        let accounts = self.accounts(None)?;
        let mut refreshed = Vec::new();
        for account in accounts.into_iter().filter(|account| account.refresh_required || account.status == "expiring") {
            if let Some(account) = self.refresh(&account.id, bridge, cancelled)? { refreshed.push(account); }
        }
        Ok(refreshed)
    }

    pub(crate) fn logout(&self, account: &str, bridge: &dyn InteractionBridge, cancelled: &dyn Fn() -> bool) -> Result<Option<Account>, WelesError> {
        self.run_operation("/auth/logout", json!({"accountId": account}), bridge, cancelled)
    }

    fn run_operation(&self, path: &str, body: Value, bridge: &dyn InteractionBridge, cancelled: &dyn Fn() -> bool) -> Result<Option<Account>, WelesError> {
        let started = self.request(reqwest::Method::POST, path, Some(&body))?;
        let operation_id = started.get("operationId").or_else(|| started.get("id")).and_then(Value::as_str).filter(|v| !v.is_empty()).ok_or_else(|| WelesError::InvalidResponse("operation id is missing".into()))?.to_string();
        let mut cursor: Option<String> = None;
        for _ in 0..MAX_POLL_EVENTS {
            if cancelled() { let _ = self.request(reqwest::Method::POST, &format!("/operations/{operation_id}/cancel"), Some(&json!({}))); return Err(WelesError::Cancelled); }
            let suffix = cursor.as_ref().map(|value| format!("?cursor={}", url::form_urlencoded::byte_serialize(value.as_bytes()).collect::<String>())).unwrap_or_default();
            let value = self.request(reqwest::Method::GET, &format!("/operations/{operation_id}{suffix}"), None)?;
            let page: OperationPage = serde_json::from_value(value).map_err(|e| WelesError::InvalidResponse(e.to_string()))?;
            cursor = page.cursor;
            for event in page.events {
                bridge.event(&event);
                match event {
                    OperationEvent::Elicit { field, prompt, secret, options } => {
                        let answer = bridge.elicit(&prompt, &options, secret).map_err(WelesError::Interaction)?;
                        self.request(reqwest::Method::POST, &format!("/operations/{operation_id}/input"), Some(&json!({"field": field, "value": answer})))?;
                    }
                    OperationEvent::Completed { account } => { BramaClient::invalidate_all(); return Ok(account); }
                    OperationEvent::Failed { code, message } => return Err(WelesError::Operation { code, message }),
                    _ => {}
                }
            }
            match page.state.as_str() {
                "completed" => { BramaClient::invalidate_all(); return Ok(None); }
                "failed" => return Err(WelesError::Operation { code: "failed".into(), message: "operation failed without an error event".into() }),
                "cancelled" => return Err(WelesError::Cancelled),
                _ => std::thread::sleep(self.poll_interval),
            }
        }
        Err(WelesError::PollLimit)
    }
}
