//! What a Brama call can fail with, and how a caller reads Brama's own
//! verdict on whether retrying helps.

use super::API_VERSION;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BramaError {
    Unconfigured,
    Transport(String),
    /// A non-2xx answer. `retryable` is Brama's own `error.retryable` field
    /// when the body is its error document, so callers read the gateway's
    /// verdict instead of searching the body text for it.
    Http {
        status: u16,
        message: String,
        retryable: Option<bool>,
    },
    /// HTTP 429. `retryable` is Brama's `error.retryable` field, read the
    /// same way as for [`BramaError::Http`], so an explicit refusal sent
    /// with 429 is not mistaken for a busy gateway.
    RateLimited {
        retry_after_ms: Option<u64>,
        retryable: Option<bool>,
    },
    InvalidCatalog(String),
    InvalidResponse(String),
    UnknownModel(String),
    UnavailableModel {
        model: String,
        reason: String,
    },
    Cancelled,
}
impl std::fmt::Display for BramaError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Unconfigured => {
                f.write_str("BRAMA_URL is required; configure the Brama model-router service URL")
            }
            Self::Transport(e) => write!(f, "Brama transport error: {e}"),
            Self::Http {
                status, message, ..
            } => write!(f, "Brama returned HTTP {status}: {message}"),
            Self::RateLimited { retry_after_ms, .. } => write!(
                f,
                "Brama rate limited the request; retry after {:?} ms",
                retry_after_ms
            ),
            Self::InvalidCatalog(e) => write!(f, "invalid Brama catalog: {e}"),
            Self::InvalidResponse(e) => write!(f, "invalid Brama response: {e}"),
            Self::UnknownModel(id) => write!(f, "model `{id}` is not in the Brama catalog"),
            Self::UnavailableModel { model, reason } => {
                write!(f, "model `{model}` is unavailable: {reason}")
            }
            Self::Cancelled => f.write_str("Brama request cancelled"),
        }
    }
}

impl BramaError {
    /// A non-2xx answer from `path`, with Brama's stated retryability read
    /// from its error document.
    pub(super) fn http(status: u16, path: &str, body: &[u8]) -> Self {
        Self::Http {
            status,
            message: format!("/{API_VERSION}{path}: {:?}", String::from_utf8_lossy(body)),
            retryable: Self::stated_retryable(body),
        }
    }

    /// Brama's `error.retryable` verdict from its error document, if the
    /// body is one.
    pub(super) fn stated_retryable(body: &[u8]) -> Option<bool> {
        serde_json::from_slice::<serde_json::Value>(body)
            .ok()
            .and_then(|document| document.pointer("/error/retryable")?.as_bool())
    }

    /// Brama said, in its error document, that retrying will not help.
    pub fn refused_outright(&self) -> bool {
        matches!(
            self,
            Self::Http {
                retryable: Some(false),
                ..
            } | Self::RateLimited {
                retryable: Some(false),
                ..
            }
        )
    }

    /// The read never got an answer about the request itself: the transport
    /// failed, the gateway was busy (429) or failed (5xx), and Brama did not
    /// say the refusal is final.
    pub fn left_unanswered(&self) -> bool {
        if self.refused_outright() {
            return false;
        }
        match self {
            Self::Transport(_) | Self::RateLimited { .. } => true,
            Self::Http { status, .. } => {
                reqwest::StatusCode::from_u16(*status).is_ok_and(|status| {
                    status == reqwest::StatusCode::TOO_MANY_REQUESTS || status.is_server_error()
                })
            }
            _ => false,
        }
    }
}
