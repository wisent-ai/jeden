use base64::{engine::general_purpose::STANDARD as BASE64, Engine as _};
use hmac::{Hmac, Mac};
use rand::Rng;
use reqwest::blocking::Client;
use serde::Serialize;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::collections::BTreeSet;
use std::path::PathBuf;
use std::sync::{
    mpsc::{self, Receiver, RecvTimeoutError, SyncSender},
    Arc,
};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

type HmacSha256 = Hmac<Sha256>;
const MAX_ROUTES: usize = 16;
const MAX_RETRY_ATTEMPTS: usize = 8;
pub(crate) const MAX_TEXT_ATTACHMENT_BYTES: usize = 256 * 1024;
const MAX_TOOL_CALLS: usize = 128;

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct RouteDescriptor {
    pub model: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub service_tier: Option<String>,
}

#[derive(Debug, Clone)]
pub struct RetryPolicy {
    pub max_attempts: usize,
    pub base_delay: Duration,
    pub max_delay: Duration,
    pub first_event_timeout: Duration,
    pub idle_timeout: Duration,
    pub jitter_ratio: f64,
}

impl Default for RetryPolicy {
    fn default() -> Self {
        Self {
            max_attempts: 3,
            base_delay: Duration::from_millis(250),
            max_delay: Duration::from_secs(8),
            first_event_timeout: Duration::from_secs(30),
            idle_timeout: Duration::from_secs(45),
            jitter_ratio: 0.2,
        }
    }
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", tag = "kind")]
pub enum RouteResult {
    RetryScheduled {
        route: RouteDescriptor,
        attempt: usize,
        delay_ms: u64,
        reason: String,
    },
    RouteChanged {
        from: RouteDescriptor,
        to: RouteDescriptor,
        reason: String,
    },
    SubscriptionChanged {
        from: crate::routing::SubscriptionTarget,
        to: crate::routing::SubscriptionTarget,
        reason: String,
    },
}

#[derive(Debug, Clone, Copy, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum StreamErrorClass {
    Cancelled,
    Timeout,
    TransientHttp,
    Network,
    ContextOverflow,
    QuotaExhausted,
    MalformedEvent,
    Incomplete,
    Permanent,
}

#[derive(Debug, Clone)]
pub struct StreamFailure {
    pub class: StreamErrorClass,
    pub message: String,
    pub route_results: Vec<RouteResult>,
    pub visible_output: bool,
}

impl std::fmt::Display for StreamFailure {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(&self.message)
    }
}

#[derive(Debug, Clone)]
pub struct StreamingCompletion {
    pub completion: Completion,
    pub route: RouteDescriptor,
    pub route_results: Vec<RouteResult>,
    pub subscription_target: Option<crate::routing::SubscriptionTarget>,
    pub subscription_decision_id: Option<String>,
}

impl StreamingCompletion {
    /// Converts transport retry/failover results into evidence attributed to
    /// the route that actually produced the completion.
    pub fn served_route_evidence(
        &self,
        decision: &crate::routing::RouteDecisionV1,
    ) -> crate::routing::ServedRouteEvidence {
        let retries = self
            .route_results
            .iter()
            .filter(|result| matches!(result, RouteResult::RetryScheduled { .. }))
            .count() as u32;
        let attempt = retries.saturating_add(1);
        if self.route.model != decision.selected_route {
            crate::routing::ServedRouteEvidence::initial(
                decision.decision_id.clone(),
                decision.selected_route.clone(),
            )
            .fallback(self.route.model.clone(), attempt)
        } else if retries > 0 {
            crate::routing::ServedRouteEvidence::initial(
                decision.decision_id.clone(),
                decision.selected_route.clone(),
            )
            .retry(attempt)
        } else {
            crate::routing::ServedRouteEvidence::initial(
                decision.decision_id.clone(),
                decision.selected_route.clone(),
            )
        }
    }
}

#[derive(Debug, Clone)]
pub(crate) enum ModelAttachment {
    Image { mime: String, bytes: Arc<[u8]> },
    Text { bytes: Arc<[u8]> },
}

impl ModelAttachment {
    pub(crate) fn image(mime: impl Into<String>, bytes: Arc<[u8]>) -> Result<Self, String> {
        let mime = mime.into();
        if !matches!(
            mime.as_str(),
            "image/png" | "image/jpeg" | "image/gif" | "image/webp"
        ) {
            return Err(format!("unsupported image attachment MIME type `{mime}`"));
        }
        if bytes.is_empty() {
            return Err("image attachment is empty".into());
        }
        Ok(Self::Image { mime, bytes })
    }

    pub(crate) fn text(bytes: Arc<[u8]>) -> Result<Self, String> {
        if bytes.is_empty() {
            return Err("text attachment is empty".into());
        }
        if bytes.len() > MAX_TEXT_ATTACHMENT_BYTES {
            return Err(format!(
                "text attachment is {} bytes; limit is {MAX_TEXT_ATTACHMENT_BYTES}",
                bytes.len()
            ));
        }
        std::str::from_utf8(bytes.as_ref())
            .map_err(|_| "text attachment is not valid UTF-8".to_string())?;
        Ok(Self::Text { bytes })
    }
}

/// Build OpenAI-compatible content parts only in the ephemeral provider copy.
/// The conversation's durable messages remain string-valued.
pub(crate) fn with_attachments(
    mut messages: Vec<Value>,
    attachments: &[ModelAttachment],
) -> Result<Vec<Value>, String> {
    if attachments.is_empty() {
        return Ok(messages);
    }
    let message = messages
        .iter_mut()
        .rev()
        .find(|message| message.get("role").and_then(Value::as_str) == Some("user"))
        .ok_or("cannot attach content: outbound messages contain no user message")?;
    let content = message
        .get_mut("content")
        .ok_or("cannot attach content: latest user message has no content")?;
    let text = match std::mem::take(content) {
        Value::String(text) => text,
        other => {
            *content = other;
            return Err("cannot attach content: latest user message content is not text".into());
        }
    };
    let mut parts = Vec::with_capacity(attachments.len().saturating_add(1));
    parts.push(json!({"type": "text", "text": text}));
    for attachment in attachments {
        match attachment {
            ModelAttachment::Image { mime, bytes } => {
                let encoded_len = (bytes.len().saturating_add(2) / 3).saturating_mul(4);
                let mut url = String::with_capacity(
                    "data:;base64,"
                        .len()
                        .saturating_add(mime.len())
                        .saturating_add(encoded_len),
                );
                url.push_str("data:");
                url.push_str(mime);
                url.push_str(";base64,");
                BASE64.encode_string(bytes.as_ref(), &mut url);
                parts.push(json!({"type": "image_url", "image_url": {"url": url}}));
            }
            ModelAttachment::Text { bytes } => {
                let text = std::str::from_utf8(bytes.as_ref())
                    .map_err(|_| "text attachment is not valid UTF-8")?;
                parts.push(json!({"type": "text", "text": text}));
            }
        }
    }
    message["content"] = Value::Array(parts);
    Ok(messages)
}

#[derive(Debug, Clone)]
pub struct ChatConfig {
    pub url: String,
    pub agent_id: String,
    pub secret: String,
    pub model: String,
    pub service_tier: String,
    pub retry: RetryPolicy,
    pub fallbacks: Vec<RouteDescriptor>,
    pub context_promotions: Vec<RouteDescriptor>,
    /// Models advertised by Brama as accepting image input.
    pub image_capable_models: BTreeSet<String>,
    pub subscription_pool: Option<crate::routing::SubscriptionPoolSnapshot>,
    pub subscription_cooldown_path: Option<PathBuf>,
    pub config_error: Option<String>,
}

#[derive(Debug, Clone)]
pub struct CompletionUsage {
    pub input_tokens: f64,
    pub output_tokens: f64,
    pub cache_read_tokens: f64,
    pub cache_write_tokens: f64,
    pub total_tokens: f64,
}

#[derive(Debug, Clone)]
pub struct Completion {
    pub content: String,
    pub usage: Option<CompletionUsage>,
}

pub fn hmac_headers(
    body: &str,
    agent_id: &str,
    secret: &str,
) -> Result<(String, String, String), String> {
    if secret.is_empty() {
        return Err("WISENT_APP_AGENT_AUTH_SECRET is required".into());
    }
    let ts = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
        .to_string();
    let body_hash = hex::encode(Sha256::digest(body.as_bytes()));
    let mut mac = HmacSha256::new_from_slice(secret.as_bytes()).map_err(|e| e.to_string())?;
    mac.update(format!("{}:{}:{}", agent_id, ts, body_hash).as_bytes());
    Ok((ts, body_hash, hex::encode(mac.finalize().into_bytes())))
}

fn tool_calls_to_action(tool_calls: &[Value]) -> Result<String, String> {
    let mut actions = Vec::new();
    for call in tool_calls {
        let name = call
            .pointer("/function/name")
            .and_then(Value::as_str)
            .unwrap_or("");
        if name.trim().is_empty() {
            return Err("model router returned tool call without function name".into());
        }
        let raw_args = call
            .pointer("/function/arguments")
            .and_then(Value::as_str)
            .unwrap_or("{}");
        let input: Value = if raw_args.trim().is_empty() {
            json!({})
        } else {
            serde_json::from_str(raw_args)
                .map_err(|e| format!("invalid tool arguments for {name}: {e}"))?
        };
        actions.push(json!({"tool": name, "input": input}));
    }
    if actions.len() == 1 {
        Ok(
            json!({"action": "tool", "tool": actions[0]["tool"], "input": actions[0]["input"]})
                .to_string(),
        )
    } else {
        Ok(json!({"action": "tools", "tools": actions}).to_string())
    }
}

pub fn chat_completion(
    config: &ChatConfig,
    messages: Vec<Value>,
    max_tokens: Option<usize>,
    tools: &[Value],
) -> Result<Completion, String> {
    if let Some(message) = &config.config_error {
        return Err(message.clone());
    }
    ensure_image_capability(config, &config.model, &messages)?;
    let mut body = json!({
        "model": config.model,
        "messages": messages,
    });
    if let Some(max_tokens) = max_tokens {
        body["max_tokens"] = json!(max_tokens);
    }
    if !config.service_tier.trim().is_empty() {
        body["service_tier"] = Value::String(config.service_tier.clone());
    }
    if !tools.is_empty() {
        body["tools"] = Value::Array(tools.to_vec());
        body["tool_choice"] = Value::String("auto".into());
    }
    let body_text = serde_json::to_string(&body).map_err(|e| e.to_string())?;
    let (ts, body_hash, sig) = hmac_headers(&body_text, &config.agent_id, &config.secret)?;
    let client = Client::builder()
        .timeout(Duration::from_secs(120))
        .build()
        .map_err(|e| e.to_string())?;
    let response = client
        .post(format!(
            "{}/v1/chat/completions",
            config.url.trim_end_matches('/')
        ))
        .header("content-type", "application/json")
        .header("x-agent-id", &config.agent_id)
        .header("x-agent-timestamp", ts)
        .header("x-agent-body-sha256", body_hash)
        .header("x-agent-signature", sig)
        .body(body_text)
        .send()
        .map_err(|e| e.to_string())?;
    let status = response.status();
    let text = response.text().map_err(|e| e.to_string())?;
    if !status.is_success() {
        return Err(format!(
            "model router {}: {}",
            status.as_u16(),
            text.chars().take(800).collect::<String>()
        ));
    }
    parse_completion_response(&text)
}

/// Parse a full (non-streamed) completion body into an action string / content.
fn value_number(value: &Value, paths: &[&str]) -> f64 {
    paths
        .iter()
        .find_map(|path| value.pointer(path).and_then(Value::as_f64))
        .unwrap_or(0.0)
}

fn usage_from_value(data: &Value) -> Option<CompletionUsage> {
    let usage = data.get("usage")?;
    let input_tokens = value_number(usage, &["/input_tokens", "/prompt_tokens", "/input"]);
    let output_tokens = value_number(usage, &["/output_tokens", "/completion_tokens", "/output"]);
    let cache_read_tokens = value_number(
        usage,
        &[
            "/cache_read_tokens",
            "/cacheRead",
            "/prompt_tokens_details/cached_tokens",
        ],
    );
    let cache_write_tokens = value_number(usage, &["/cache_write_tokens", "/cacheWrite"]);
    let total_tokens = value_number(usage, &["/total_tokens", "/totalTokens", "/total"]);
    let total_tokens = if total_tokens > 0.0 {
        total_tokens
    } else {
        input_tokens + output_tokens + cache_read_tokens + cache_write_tokens
    };
    if input_tokens == 0.0
        && output_tokens == 0.0
        && cache_read_tokens == 0.0
        && cache_write_tokens == 0.0
        && total_tokens == 0.0
    {
        None
    } else {
        Some(CompletionUsage {
            input_tokens,
            output_tokens,
            cache_read_tokens,
            cache_write_tokens,
            total_tokens,
        })
    }
}

/// Parse a full (non-streamed) completion body into an action string / content.
fn parse_completion_response(text: &str) -> Result<Completion, String> {
    let data: Value =
        serde_json::from_str(text).map_err(|e| format!("invalid model router JSON: {e}"))?;
    let usage = usage_from_value(&data);
    let message = data
        .pointer("/choices/0/message")
        .ok_or("model router returned no message")?;
    let finish_reason = data
        .pointer("/choices/0/finish_reason")
        .and_then(Value::as_str)
        .unwrap_or("");
    if matches!(finish_reason, "length" | "max_tokens") {
        return Err("model response incomplete: length".into());
    }
    let tool_calls = message
        .get("tool_calls")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    if !tool_calls.is_empty() {
        return Ok(Completion {
            content: tool_calls_to_action(&tool_calls)?,
            usage,
        });
    }
    let content = message.get("content").and_then(Value::as_str).unwrap_or("");
    if content.trim().is_empty() {
        return Err("model router returned no message content".into());
    }
    Ok(Completion {
        content: content.to_string(),
        usage,
    })
}

fn subscription_provider_for_model(model: &str) -> Option<&'static str> {
    match model.trim().to_ascii_lowercase().as_str() {
        "claude-code-subscription" | "claude-opus-4-7" => Some("claude_code"),
        "codex-subscription" => Some("codex"),
        "kimi-subscription" => Some("kimi"),
        "opencode-subscription" => Some("opencode"),
        _ => None,
    }
}

fn subscription_targets_for_route<'a>(
    decision: Option<&'a crate::routing::RouteDecisionV2>,
    model: &str,
) -> Vec<Option<&'a crate::routing::SubscriptionTarget>> {
    match (decision, subscription_provider_for_model(model)) {
        (Some(decision), Some(provider)) => decision
            .targets
            .iter()
            .filter(|target| target.provider_id == provider)
            .map(Some)
            .collect(),
        _ => vec![None],
    }
}

/// Streaming chat completion. Requests SSE (`stream: true`); for each content
/// delta it calls `on_delta`. Falls back to whole-body parsing if the endpoint
/// ignores `stream` and returns a normal JSON completion. Tool-call responses
/// are accumulated and returned as an action string (no partial tool deltas are
/// surfaced). Returns the same action/content string as `chat_completion`.
pub fn chat_completion_streaming(
    config: &ChatConfig,
    messages: Vec<Value>,
    max_tokens: Option<usize>,
    tools: &[Value],
    on_delta: &mut dyn FnMut(&str) -> bool,
    cancelled: &dyn Fn() -> bool,
) -> Result<StreamingCompletion, StreamFailure> {
    if let Some(message) = &config.config_error {
        return Err(stream_failure(
            StreamErrorClass::Permanent,
            message.clone(),
            Vec::new(),
            false,
        ));
    }
    let primary = RouteDescriptor {
        model: config.model.clone(),
        service_tier: nonempty(&config.service_tier),
    };
    let mut routes = Vec::with_capacity((config.fallbacks.len() + 1).min(MAX_ROUTES));
    routes.push(primary.clone());
    for route in config.fallbacks.iter().take(MAX_ROUTES.saturating_sub(1)) {
        if !routes.contains(route) {
            routes.push(route.clone());
        }
    }

    let attempts = config.retry.max_attempts.clamp(1, MAX_RETRY_ATTEMPTS);
    let (request_id, idempotency_key, sticky_key) = logical_request_keys(&primary, &messages);
    let cooldown_store = match config.subscription_cooldown_path.as_ref() {
        Some(path) => Some(
            crate::routing::CooldownStore::open(path).map_err(|message| {
                stream_failure(
                    StreamErrorClass::Permanent,
                    format!("cannot open subscription cooldown store: {message}"),
                    Vec::new(),
                    false,
                )
            })?,
        ),
        None => None,
    };
    let subscription_decision = match config.subscription_pool.as_ref() {
        Some(pool) if !pool.targets.is_empty() => {
            let required = ["chat".to_string()].into_iter().collect();
            let now_ms = epoch_millis();
            Some(
                crate::routing::RouteDecisionV2::freeze(
                    pool,
                    request_id,
                    idempotency_key,
                    &sticky_key,
                    &required,
                    now_ms,
                    |identity| {
                        cooldown_store.as_ref().is_some_and(|store| {
                            store.is_cooling_down(identity, now_ms).unwrap_or(true)
                        })
                    },
                )
                .map_err(|message| {
                    stream_failure(StreamErrorClass::QuotaExhausted, message, Vec::new(), false)
                })?,
            )
        }
        _ => None,
    };
    let max_target_count = subscription_decision
        .as_ref()
        .map_or(1, |decision| decision.targets.len());
    let mut route_results = Vec::with_capacity(
        routes
            .len()
            .saturating_mul(max_target_count)
            .saturating_mul(attempts),
    );
    let mut last_error: Option<AttemptError> = None;
    let mut exhausted_targets = std::collections::BTreeSet::new();
    for (route_index, route) in routes.iter().enumerate() {
        if let Err(message) = ensure_image_capability(config, &route.model, &messages) {
            if let Some(next) = routes.get(route_index + 1) {
                route_results.push(RouteResult::RouteChanged {
                    from: route.clone(),
                    to: next.clone(),
                    reason: message.clone(),
                });
            }
            last_error = Some(AttemptError::permanent(message));
            continue;
        }
        let route_targets =
            subscription_targets_for_route(subscription_decision.as_ref(), &route.model);
        if route_targets.is_empty() {
            last_error = Some(AttemptError {
                class: StreamErrorClass::QuotaExhausted,
                message: format!(
                    "no eligible subscription target for model '{}'",
                    route.model
                ),
                retry_after: None,
                visible_output: false,
            });
            continue;
        }
        for target_index in 0..route_targets.len() {
            let target = route_targets[target_index];
            if target.is_some_and(|target| exhausted_targets.contains(&target.identity())) {
                continue;
            }
            for attempt in 1..=attempts {
                if cancelled() {
                    return Err(stream_failure(
                        StreamErrorClass::Cancelled,
                        "Turn cancelled.".into(),
                        route_results,
                        false,
                    ));
                }
                match streaming_attempt(
                    config,
                    route,
                    &messages,
                    max_tokens,
                    tools,
                    target,
                    target.and(subscription_decision.as_ref()),
                    on_delta,
                    cancelled,
                ) {
                    Ok(completion) => {
                        return Ok(StreamingCompletion {
                            completion,
                            route: route.clone(),
                            route_results,
                            subscription_target: target.cloned(),
                            subscription_decision_id: target.and(
                                subscription_decision
                                    .as_ref()
                                    .map(|decision| decision.decision_id.clone()),
                            ),
                        });
                    }
                    Err(error) => {
                        if error.class == StreamErrorClass::QuotaExhausted && !error.visible_output
                        {
                            if let Some(target) = target {
                                exhausted_targets.insert(target.identity());
                            }
                            if let (Some(store), Some(target)) = (cooldown_store.as_ref(), target) {
                                let now_ms = epoch_millis();
                                let delay = error.retry_after.unwrap_or(Duration::from_secs(60));
                                let until_ms = now_ms.saturating_add(duration_millis(delay).max(1));
                                if let Err(message) =
                                    store.record(target.identity(), until_ms, now_ms)
                                {
                                    return Err(stream_failure(
                                        StreamErrorClass::Permanent,
                                        format!("cannot persist subscription cooldown: {message}"),
                                        route_results,
                                        false,
                                    ));
                                }
                            }
                            last_error = Some(error);
                            break;
                        }
                        let retryable = error.is_transient() && !error.visible_output;
                        if !retryable {
                            return Err(stream_failure(
                                error.class,
                                error.message,
                                route_results,
                                error.visible_output,
                            ));
                        }
                        if attempt < attempts {
                            let delay = retry_delay(&config.retry, attempt, error.retry_after);
                            route_results.push(RouteResult::RetryScheduled {
                                route: route.clone(),
                                attempt: attempt + 1,
                                delay_ms: duration_millis(delay),
                                reason: error.message.clone(),
                            });
                            cancellable_sleep(delay, cancelled).map_err(|message| {
                                stream_failure(
                                    StreamErrorClass::Cancelled,
                                    message,
                                    route_results.clone(),
                                    false,
                                )
                            })?;
                        }
                        last_error = Some(error);
                    }
                }
            }
            if let (Some(from), Some(to)) = (
                route_targets.get(target_index).copied().flatten(),
                route_targets.get(target_index + 1).copied().flatten(),
            ) {
                route_results.push(RouteResult::SubscriptionChanged {
                    from: from.clone(),
                    to: to.clone(),
                    reason: "subscription quota or transient attempts exhausted".into(),
                });
            }
        }
        if let Some(next) = routes.get(route_index + 1) {
            route_results.push(RouteResult::RouteChanged {
                from: route.clone(),
                to: next.clone(),
                reason: "subscription targets and transient attempts exhausted".into(),
            });
        }
    }
    let error = last_error.unwrap_or_else(|| AttemptError::permanent("no model route configured"));
    Err(stream_failure(
        error.class,
        error.message,
        route_results,
        error.visible_output,
    ))
}

#[derive(Debug)]
struct AttemptError {
    class: StreamErrorClass,
    message: String,
    retry_after: Option<Duration>,
    visible_output: bool,
}

impl AttemptError {
    fn permanent(message: impl Into<String>) -> Self {
        Self {
            class: StreamErrorClass::Permanent,
            message: message.into(),
            retry_after: None,
            visible_output: false,
        }
    }

    fn is_transient(&self) -> bool {
        matches!(
            self.class,
            StreamErrorClass::Timeout | StreamErrorClass::TransientHttp | StreamErrorClass::Network
        )
    }
}

fn stream_failure(
    class: StreamErrorClass,
    message: String,
    route_results: Vec<RouteResult>,
    visible_output: bool,
) -> StreamFailure {
    StreamFailure {
        class,
        message,
        route_results,
        visible_output,
    }
}

fn messages_use_image_input(messages: &[Value]) -> bool {
    messages.iter().any(|message| {
        message
            .get("content")
            .and_then(Value::as_array)
            .is_some_and(|parts| {
                parts
                    .iter()
                    .any(|part| part.get("type").and_then(Value::as_str) == Some("image_url"))
            })
    })
}

fn ensure_image_capability(
    config: &ChatConfig,
    model: &str,
    messages: &[Value],
) -> Result<(), String> {
    if messages_use_image_input(messages) && !config.image_capable_models.contains(model) {
        return Err(format!(
            "model `{model}` does not advertise image input support; choose an image-capable model"
        ));
    }
    Ok(())
}

enum WireMessage {
    Headers {
        status: u16,
        content_type: String,
        retry_after: Option<Duration>,
    },
    FullBody(Result<String, String>),
    Line(Result<String, String>),
    Eof,
    Network(String),
}

fn build_streaming_body(
    route: &RouteDescriptor,
    messages: &[Value],
    max_tokens: Option<usize>,
    tools: &[Value],
    target: Option<&crate::routing::SubscriptionTarget>,
    decision: Option<&crate::routing::RouteDecisionV2>,
) -> Result<Value, String> {
    let mut body = json!({
        "model": route.model,
        "messages": messages,
        "stream": true,
        "stream_options": {"include_usage": true},
    });
    if let Some(max_tokens) = max_tokens {
        body["max_tokens"] = json!(max_tokens);
    }
    if let Some(service_tier) = route.service_tier.as_ref() {
        body["service_tier"] = json!(service_tier);
    }
    if !tools.is_empty() {
        body["tools"] = Value::Array(tools.to_vec());
        body["tool_choice"] = Value::String("auto".into());
    }
    if let Some(target) = target {
        body["billingTarget"] = serde_json::to_value(target).map_err(|error| error.to_string())?;
    }
    if let Some(decision) = decision {
        body["subscriptionDecisionId"] = Value::String(decision.decision_id.clone());
        body["requestId"] = Value::String(decision.request_id.clone());
        body["idempotencyKey"] = Value::String(decision.idempotency_key.clone());
    }
    Ok(body)
}

fn streaming_attempt(
    config: &ChatConfig,
    route: &RouteDescriptor,
    messages: &[Value],
    max_tokens: Option<usize>,
    tools: &[Value],
    target: Option<&crate::routing::SubscriptionTarget>,
    decision: Option<&crate::routing::RouteDecisionV2>,
    on_delta: &mut dyn FnMut(&str) -> bool,
    cancelled: &dyn Fn() -> bool,
) -> Result<Completion, AttemptError> {
    let body = build_streaming_body(route, messages, max_tokens, tools, target, decision)
        .map_err(AttemptError::permanent)?;
    let body_text =
        serde_json::to_string(&body).map_err(|error| AttemptError::permanent(error.to_string()))?;
    let (sender, receiver) = mpsc::sync_channel(16);
    spawn_openai_stream_adapter(config, body_text, sender)?;

    let mut state = OpenAiStreamState::default();
    let first_deadline = Instant::now() + config.retry.first_event_timeout;
    let mut idle_deadline = first_deadline;
    let mut first_event_seen = false;
    let mut content_type = String::new();
    loop {
        let deadline = if first_event_seen {
            idle_deadline
        } else {
            first_deadline
        };
        let message = recv_until(&receiver, deadline, cancelled).map_err(|class| AttemptError {
            class,
            message: match class {
                StreamErrorClass::Cancelled => "Turn cancelled.".into(),
                StreamErrorClass::Network => "model stream adapter disconnected".into(),
                StreamErrorClass::Timeout if first_event_seen => "model stream idle timeout".into(),
                StreamErrorClass::Timeout => "model stream first-event timeout".into(),
                _ => "model stream receive failure".into(),
            },
            retry_after: None,
            visible_output: state.visible_output,
        })?;
        idle_deadline = Instant::now() + config.retry.idle_timeout;
        match message {
            WireMessage::Headers {
                status,
                content_type: kind,
                retry_after,
            } => {
                content_type = kind;
                if !(200..300).contains(&status) {
                    let body = match recv_until(&receiver, idle_deadline, cancelled) {
                        Ok(WireMessage::FullBody(Ok(body))) => body,
                        Ok(WireMessage::FullBody(Err(error))) | Ok(WireMessage::Network(error)) => {
                            error
                        }
                        _ => String::new(),
                    };
                    return Err(http_error(status, body, retry_after));
                }
            }
            WireMessage::FullBody(result) => {
                let text = result.map_err(|message| AttemptError {
                    class: StreamErrorClass::Network,
                    message,
                    retry_after: None,
                    visible_output: false,
                })?;
                if content_type.contains("event-stream") {
                    return Err(malformed(
                        "event-stream response arrived without SSE framing",
                        false,
                    ));
                }
                return parse_completion_response(&text)
                    .map_err(|message| malformed(message, false));
            }
            WireMessage::Line(result) => {
                let line = result.map_err(|message| AttemptError {
                    class: StreamErrorClass::Network,
                    message,
                    retry_after: None,
                    visible_output: state.visible_output,
                })?;
                if let Some(payload) = state
                    .sse
                    .push_line(&line)
                    .map_err(|message| malformed(message, state.visible_output))?
                {
                    first_event_seen = true;
                    if state.apply_payload(&payload, on_delta)? {
                        return state.finish();
                    }
                }
            }
            WireMessage::Eof => {
                if let Some(payload) = state
                    .sse
                    .finish()
                    .map_err(|message| malformed(message, state.visible_output))?
                {
                    if state.apply_payload(&payload, on_delta)? {
                        return state.finish();
                    }
                }
                return Err(AttemptError {
                    class: StreamErrorClass::Network,
                    message: "model stream ended before [DONE]".into(),
                    retry_after: None,
                    visible_output: state.visible_output,
                });
            }
            WireMessage::Network(message) => {
                return Err(AttemptError {
                    class: StreamErrorClass::Network,
                    message,
                    retry_after: None,
                    visible_output: state.visible_output,
                });
            }
        }
    }
}

fn spawn_openai_stream_adapter(
    config: &ChatConfig,
    body_text: String,
    sender: SyncSender<WireMessage>,
) -> Result<(), AttemptError> {
    use std::io::{BufRead, BufReader};
    let (ts, body_hash, signature) = hmac_headers(&body_text, &config.agent_id, &config.secret)
        .map_err(AttemptError::permanent)?;
    let url = format!("{}/v1/chat/completions", config.url.trim_end_matches('/'));
    let agent_id = config.agent_id.clone();
    std::thread::Builder::new()
        .name("model-stream-adapter".into())
        .spawn(move || {
            let client = match Client::builder().timeout(Duration::from_secs(300)).build() {
                Ok(client) => client,
                Err(error) => {
                    let _ = sender.send(WireMessage::Network(error.to_string()));
                    return;
                }
            };
            let response = match client
                .post(url)
                .header("content-type", "application/json")
                .header("accept", "text/event-stream")
                .header("x-agent-id", agent_id)
                .header("x-agent-timestamp", ts)
                .header("x-agent-body-sha256", body_hash)
                .header("x-agent-signature", signature)
                .body(body_text)
                .send()
            {
                Ok(response) => response,
                Err(error) => {
                    let _ = sender.send(WireMessage::Network(error.to_string()));
                    return;
                }
            };
            let status = response.status().as_u16();
            let content_type = response
                .headers()
                .get("content-type")
                .and_then(|value| value.to_str().ok())
                .unwrap_or("")
                .to_string();
            let retry_after = response
                .headers()
                .get("retry-after")
                .and_then(|value| value.to_str().ok())
                .and_then(parse_retry_after);
            if sender
                .send(WireMessage::Headers {
                    status,
                    content_type: content_type.clone(),
                    retry_after,
                })
                .is_err()
            {
                return;
            }
            if !(200..300).contains(&status) || !content_type.contains("event-stream") {
                let result = response.text().map_err(|error| error.to_string());
                let _ = sender.send(WireMessage::FullBody(result));
                return;
            }
            for line in BufReader::new(response).lines() {
                if sender
                    .send(WireMessage::Line(line.map_err(|error| error.to_string())))
                    .is_err()
                {
                    return;
                }
            }
            let _ = sender.send(WireMessage::Eof);
        })
        .map_err(|error| {
            AttemptError::permanent(format!("cannot start model stream adapter: {error}"))
        })?;
    Ok(())
}

#[derive(Default)]
struct SseDecoder {
    data: String,
}

impl SseDecoder {
    fn push_line(&mut self, line: &str) -> Result<Option<String>, String> {
        if line.is_empty() {
            return self.take_event();
        }
        if line.starts_with(':') {
            return Ok(None);
        }
        let (field, value) = line.split_once(':').unwrap_or((line, ""));
        let value = value.strip_prefix(' ').unwrap_or(value);
        match field {
            "data" => {
                if !self.data.is_empty() {
                    self.data.push('\n');
                }
                self.data.push_str(value);
            }
            "event" | "id" | "retry" => {}
            _ => return Err(format!("malformed SSE field: {field}")),
        }
        Ok(None)
    }

    fn finish(&mut self) -> Result<Option<String>, String> {
        self.take_event()
    }

    fn take_event(&mut self) -> Result<Option<String>, String> {
        if self.data.is_empty() {
            return Ok(None);
        }
        Ok(Some(std::mem::take(&mut self.data)))
    }
}

#[derive(Debug)]
enum OpenAiStreamEvent {
    Text(String),
    Thinking,
    ToolCalls(Vec<Value>),
    Usage(CompletionUsage),
    Metadata,
    Incomplete,
    Done,
}

#[derive(Default)]
struct OpenAiStreamState {
    sse: SseDecoder,
    content: String,
    tool_calls: Vec<Value>,
    usage: Option<CompletionUsage>,
    visible_output: bool,
}

impl OpenAiStreamState {
    fn apply_payload(
        &mut self,
        payload: &str,
        on_delta: &mut dyn FnMut(&str) -> bool,
    ) -> Result<bool, AttemptError> {
        for event in parse_stream_events(payload)
            .map_err(|message| malformed(message, self.visible_output))?
        {
            match event {
                OpenAiStreamEvent::Text(piece) => {
                    self.content.push_str(&piece);
                    self.visible_output |= on_delta(&piece);
                }
                OpenAiStreamEvent::Thinking | OpenAiStreamEvent::Metadata => {}
                OpenAiStreamEvent::ToolCalls(calls) => {
                    accumulate_tool_call_deltas(&mut self.tool_calls, &calls)
                        .map_err(|message| malformed(message, self.visible_output))?;
                }
                OpenAiStreamEvent::Usage(usage) => self.usage = Some(usage),
                OpenAiStreamEvent::Incomplete => {
                    return Err(AttemptError {
                        class: StreamErrorClass::Incomplete,
                        message: "model response incomplete: length".into(),
                        retry_after: None,
                        visible_output: self.visible_output,
                    });
                }
                OpenAiStreamEvent::Done => return Ok(true),
            }
        }
        Ok(false)
    }

    fn finish(&mut self) -> Result<Completion, AttemptError> {
        if !self.tool_calls.is_empty() {
            return Ok(Completion {
                content: tool_calls_to_action(&self.tool_calls).map_err(AttemptError::permanent)?,
                usage: self.usage.take(),
            });
        }
        if self.content.trim().is_empty() {
            return Err(AttemptError::permanent(
                "model router returned no message content",
            ));
        }
        Ok(Completion {
            content: std::mem::take(&mut self.content),
            usage: self.usage.take(),
        })
    }
}

fn parse_stream_events(payload: &str) -> Result<Vec<OpenAiStreamEvent>, String> {
    if payload.trim() == "[DONE]" {
        return Ok(vec![OpenAiStreamEvent::Done]);
    }
    let chunk: Value = serde_json::from_str(payload)
        .map_err(|error| format!("malformed model stream JSON: {error}"))?;
    if let Some(error) = chunk.get("error") {
        return Err(format!("model stream error event: {error}"));
    }
    let mut events = Vec::with_capacity(4);
    if let Some(usage) = usage_from_value(&chunk) {
        events.push(OpenAiStreamEvent::Usage(usage));
    }
    let choices = chunk
        .get("choices")
        .and_then(Value::as_array)
        .ok_or("model stream event has no choices array")?;
    if choices.is_empty() {
        if events.is_empty() {
            return Err("model stream event has neither choices nor usage".into());
        }
        return Ok(events);
    }
    let choice = choices
        .first()
        .ok_or("model stream event has no first choice")?;
    if matches!(
        choice.get("finish_reason").and_then(Value::as_str),
        Some("length" | "max_tokens")
    ) {
        events.push(OpenAiStreamEvent::Incomplete);
    }
    let delta = choice
        .get("delta")
        .ok_or("model stream choice has no delta")?;
    let mut recognized = false;
    if let Some(content) = delta.get("content") {
        match content {
            Value::Null => {}
            Value::String(content) if !content.is_empty() => {
                events.push(OpenAiStreamEvent::Text(content.clone()));
            }
            Value::String(_) => {}
            _ => return Err("model stream content delta is not a string or null".into()),
        }
        recognized = true;
    }
    if let Some(reasoning) = delta
        .get("reasoning_content")
        .or_else(|| delta.get("reasoning"))
    {
        if !reasoning.is_null() && !reasoning.is_string() {
            return Err("model stream reasoning delta is not a string or null".into());
        }
        events.push(OpenAiStreamEvent::Thinking);
        recognized = true;
    }
    if let Some(calls) = delta.get("tool_calls") {
        events.push(OpenAiStreamEvent::ToolCalls(
            calls
                .as_array()
                .ok_or("model stream tool_calls delta is not an array")?
                .clone(),
        ));
        recognized = true;
    }
    if delta.get("role").is_some() || choice.get("finish_reason").is_some() {
        events.push(OpenAiStreamEvent::Metadata);
        recognized = true;
    }
    if !recognized && events.is_empty() {
        return Err("unrecognized model stream event".into());
    }
    Ok(events)
}

fn malformed(message: impl Into<String>, visible_output: bool) -> AttemptError {
    AttemptError {
        class: StreamErrorClass::MalformedEvent,
        message: message.into(),
        retry_after: None,
        visible_output,
    }
}

fn http_error(status: u16, body: String, retry_after: Option<Duration>) -> AttemptError {
    let normalized = body.to_ascii_lowercase();
    let quota_exhausted = status == 402
        || (status == 429 && (normalized.contains("quota") || normalized.contains("subscription")));
    let class = if quota_exhausted {
        StreamErrorClass::QuotaExhausted
    } else if matches!(status, 408 | 409 | 425 | 429) || (500..600).contains(&status) {
        StreamErrorClass::TransientHttp
    } else if is_context_overflow_body(&body) {
        StreamErrorClass::ContextOverflow
    } else {
        StreamErrorClass::Permanent
    };
    AttemptError {
        class,
        message: format!(
            "model router {status}: {}",
            body.chars().take(800).collect::<String>()
        ),
        retry_after,
        visible_output: false,
    }
}

fn is_context_overflow_body(body: &str) -> bool {
    let lower = body.to_ascii_lowercase();
    lower.contains("context length")
        || lower.contains("context window")
        || lower.contains("maximum context")
        || lower.contains("too many tokens")
        || lower.contains("tokens exceed")
}

fn recv_until(
    receiver: &Receiver<WireMessage>,
    deadline: Instant,
    cancelled: &dyn Fn() -> bool,
) -> Result<WireMessage, StreamErrorClass> {
    loop {
        if cancelled() {
            return Err(StreamErrorClass::Cancelled);
        }
        let now = Instant::now();
        if now >= deadline {
            return Err(StreamErrorClass::Timeout);
        }
        let wait = deadline
            .saturating_duration_since(now)
            .min(Duration::from_millis(25));
        match receiver.recv_timeout(wait) {
            Ok(message) => return Ok(message),
            Err(RecvTimeoutError::Timeout) => {}
            Err(RecvTimeoutError::Disconnected) => return Err(StreamErrorClass::Network),
        }
    }
}

fn retry_delay(policy: &RetryPolicy, attempt: usize, retry_after: Option<Duration>) -> Duration {
    if let Some(delay) = retry_after {
        return delay;
    }
    let exponent = (attempt.saturating_sub(1)).min(20) as u32;
    let base_ms = policy
        .base_delay
        .as_millis()
        .saturating_mul(1u128 << exponent);
    let capped_ms = base_ms.min(policy.max_delay.as_millis()) as f64;
    let jitter = policy.jitter_ratio.clamp(0.0, 1.0);
    let factor = rand::thread_rng().gen_range((1.0 - jitter)..=(1.0 + jitter));
    Duration::from_millis((capped_ms * factor).round().max(0.0) as u64)
}

fn cancellable_sleep(delay: Duration, cancelled: &dyn Fn() -> bool) -> Result<(), String> {
    let deadline = Instant::now() + delay;
    while Instant::now() < deadline {
        if cancelled() {
            return Err("Turn cancelled.".into());
        }
        std::thread::sleep(
            deadline
                .saturating_duration_since(Instant::now())
                .min(Duration::from_millis(25)),
        );
    }
    Ok(())
}

fn parse_retry_after(value: &str) -> Option<Duration> {
    if let Ok(seconds) = value.trim().parse::<u64>() {
        return Some(Duration::from_secs(seconds));
    }
    let mut fields = value.split_whitespace();
    let weekday = fields.next()?;
    if !weekday.ends_with(',') {
        return None;
    }
    let day = fields.next()?.parse::<u32>().ok()?;
    let month = match fields.next()? {
        "Jan" => 1,
        "Feb" => 2,
        "Mar" => 3,
        "Apr" => 4,
        "May" => 5,
        "Jun" => 6,
        "Jul" => 7,
        "Aug" => 8,
        "Sep" => 9,
        "Oct" => 10,
        "Nov" => 11,
        "Dec" => 12,
        _ => return None,
    };
    let year = fields.next()?.parse::<i64>().ok()?;
    let mut clock = fields.next()?.split(':');
    let hour = clock.next()?.parse::<u32>().ok()?;
    let minute = clock.next()?.parse::<u32>().ok()?;
    let second = clock.next()?.parse::<u32>().ok()?;
    if clock.next().is_some()
        || fields.next()? != "GMT"
        || fields.next().is_some()
        || !(1..=31).contains(&day)
        || hour > 23
        || minute > 59
        || second > 60
    {
        return None;
    }
    let target = days_from_civil(year, month, day)
        .checked_mul(86_400)?
        .checked_add(i64::from(hour) * 3_600 + i64::from(minute) * 60 + i64::from(second))?;
    let now = SystemTime::now().duration_since(UNIX_EPOCH).ok()?.as_secs();
    let target = u64::try_from(target).ok()?;
    Some(Duration::from_secs(target.saturating_sub(now)))
}

fn days_from_civil(year: i64, month: u32, day: u32) -> i64 {
    let year = year - if month <= 2 { 1 } else { 0 };
    let era = if year >= 0 { year } else { year - 399 } / 400;
    let year_of_era = year - era * 400;
    let shifted_month = i64::from(month) + if month > 2 { -3 } else { 9 };
    let day_of_year = (153 * shifted_month + 2) / 5 + i64::from(day) - 1;
    let day_of_era = year_of_era * 365 + year_of_era / 4 - year_of_era / 100 + day_of_year;
    era * 146_097 + day_of_era - 719_468
}

struct DigestSink<'a>(&'a mut Sha256);

impl std::io::Write for DigestSink<'_> {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        self.0.update(bytes);
        Ok(bytes.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

fn logical_request_keys(route: &RouteDescriptor, messages: &[Value]) -> (String, String, String) {
    let mut hasher = Sha256::new();
    if serde_json::to_writer(DigestSink(&mut hasher), &(route, messages)).is_err() {
        hasher = Sha256::new();
    }
    let digest = hex::encode(hasher.finalize());
    (
        format!("request-{digest}"),
        format!("completion-{digest}"),
        format!("session-{digest}"),
    )
}

fn epoch_millis() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
        .min(u128::from(u64::MAX)) as u64
}

fn duration_millis(duration: Duration) -> u64 {
    duration.as_millis().min(u128::from(u64::MAX)) as u64
}

fn nonempty(value: &str) -> Option<String> {
    (!value.trim().is_empty()).then(|| value.to_string())
}

/// Merge streaming tool-call deltas (indexed) into a growing list.
fn accumulate_tool_call_deltas(acc: &mut Vec<Value>, deltas: &[Value]) -> Result<(), String> {
    for delta in deltas {
        let raw_index = delta.get("index").and_then(Value::as_u64).unwrap_or(0);
        let index =
            usize::try_from(raw_index).map_err(|_| "tool call index exceeds platform size")?;
        if index >= MAX_TOOL_CALLS {
            return Err(format!(
                "tool call index {index} exceeds limit {MAX_TOOL_CALLS}"
            ));
        }
        while acc.len() <= index {
            acc.push(json!({"function": {"name": "", "arguments": ""}}));
        }
        let slot = &mut acc[index];
        if let Some(name) = delta.pointer("/function/name").and_then(Value::as_str) {
            if !name.is_empty() {
                let current = slot
                    .pointer("/function/name")
                    .and_then(Value::as_str)
                    .unwrap_or("");
                let updated = format!("{current}{name}");
                slot["function"]["name"] = Value::String(updated);
            }
        }
        if let Some(arguments) = delta.pointer("/function/arguments").and_then(Value::as_str) {
            let current = slot
                .pointer("/function/arguments")
                .and_then(Value::as_str)
                .unwrap_or("");
            let updated = format!("{current}{arguments}");
            slot["function"]["arguments"] = Value::String(updated);
        }
    }
    Ok(())
}
