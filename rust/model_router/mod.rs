use base64::{engine::general_purpose::STANDARD as BASE64, Engine as _};
use hmac::{Hmac, Mac};
use serde::Serialize;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::collections::BTreeSet;
use std::sync::{
    mpsc::{self, Receiver, Sender},
    Arc,
};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

mod attachments;
mod buffered;
mod completion;
pub(crate) mod lease;
mod stream;
mod transport;

pub(crate) use attachments::{with_attachments, ModelAttachment};
pub use buffered::chat_completion;
pub use completion::hmac_headers;
use completion::*;
pub use stream::chat_completion_streaming;
use stream::*;

type HmacSha256 = Hmac<Sha256>;
pub(crate) const AUTOMATIC_MODEL_ROUTE: &str = "any";
pub(crate) const VISION_MODEL_ROUTE: &str = "any-vision-capable";

pub(crate) fn is_virtual_model_route(model: &str) -> bool {
    matches!(model, AUTOMATIC_MODEL_ROUTE | VISION_MODEL_ROUTE)
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct RouteDescriptor {
    pub model: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub service_tier: Option<String>,
}

/// How many times one route is tried. Unstated (no `modelRouting.retry`),
/// each route is tried once and a failure moves to the next route.
#[derive(Debug, Clone)]
pub struct RetryPolicy {
    pub max_attempts: usize,
}

impl Default for RetryPolicy {
    fn default() -> Self {
        Self {
            max_attempts: std::num::NonZeroUsize::MIN.get(),
        }
    }
}

// Boxing `RouteChanged`'s descriptors would rewrite every construction and
// every match arm across the router for a value that is built once per retry.
#[allow(clippy::large_enum_variant)]
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
    /// The session's account moved: the call on `from` failed before any
    /// output and Brama leased `to`, an account with capacity.
    SubscriptionChanged {
        from: lease::SessionLease,
        to: lease::SessionLease,
        reason: String,
    },
}

#[derive(Debug, Clone, Copy, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum StreamErrorClass {
    Cancelled,
    TransientHttp,
    Network,
    ContextOverflow,
    QuotaExhausted,
    MalformedEvent,
    Incomplete,
    EmptyResponse,
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
    /// The Brama lease the completion ran on, when its route is leased.
    pub lease: Option<lease::SessionLease>,
}

#[derive(Debug, Clone)]
pub struct ChatConfig {
    pub url: String,
    /// Provenance of the selected endpoint, retained with this configuration.
    pub endpoint_origin: String,
    pub endpoint_via_stado: bool,
    pub bearer_token: String,
    /// This route belongs to the private gateway this process owns. Resolve
    /// its current endpoint and bearer for each request, not from this snapshot.
    pub own_gateway: bool,
    pub agent_id: String,
    pub secret: String,
    pub model: String,
    pub service_tier: String,
    pub retry: RetryPolicy,
    pub fallbacks: Vec<RouteDescriptor>,
    pub context_promotions: Vec<RouteDescriptor>,
    /// Models advertised by Brama as accepting image input.
    pub image_capable_models: BTreeSet<String>,
    /// Whether calls go to Brama, which leases accounts; false for a
    /// provider named by `JEDEN_MODEL_ENDPOINT`.
    pub via_brama: bool,
    /// The Jeden session the calls belong to; a leased route takes its
    /// account for this session.
    pub session_id: Option<String>,
    /// The lease the program that started this session holds for it
    /// (`JEDEN_LEASE`); the session runs on it and takes none of its own.
    pub inherited_lease: Option<lease::SessionLease>,
    pub config_error: Option<String>,
    /// `config_error` is the catalog read that got no answer about the
    /// request (`BramaError::left_unanswered`), set where the typed error is
    /// still in hand so no caller reads that from the sentence.
    pub catalog_unanswered: bool,
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
