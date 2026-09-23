use base64::{engine::general_purpose::STANDARD as BASE64, Engine as _};
use hmac::{Hmac, Mac};
use rand::Rng;
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

mod attachments;
mod completion;
mod stream;

pub(crate) use attachments::{with_attachments, ModelAttachment};
pub use completion::{chat_completion, hmac_headers};
pub use stream::chat_completion_streaming;
use completion::*;
use stream::*;

type HmacSha256 = Hmac<Sha256>;
const MAX_ROUTES: usize = 16;
const MAX_RETRY_ATTEMPTS: usize = 8;
pub(crate) const AUTOMATIC_MODEL_ROUTE: &str = "any";
pub(crate) const VISION_MODEL_ROUTE: &str = "any-vision-capable";

pub(crate) fn is_virtual_model_route(model: &str) -> bool {
    matches!(model, AUTOMATIC_MODEL_ROUTE | VISION_MODEL_ROUTE)
}
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
    pub jitter_ratio: f64,
}

impl Default for RetryPolicy {
    fn default() -> Self {
        Self {
            max_attempts: 3,
            base_delay: Duration::from_secs(2),
            max_delay: Duration::from_secs(8),
            jitter_ratio: 0.2,
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
pub struct ChatConfig {
    pub url: String,
    pub bearer_token: String,
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

