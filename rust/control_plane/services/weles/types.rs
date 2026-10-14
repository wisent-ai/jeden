//! What the platform billing service talks about, and every way it can
//! refuse.
//!
//! Split out of `control_plane/services/weles.rs`, which had grown past the
//! module line cap.

use serde::{Deserialize, Serialize};

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

/// Where a Weles operation is, as the Weles API spells it.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum OperationState {
    Pending,
    Running,
    Completed,
    Failed,
    Cancelled,
}

impl OperationState {
    /// Whether the operation has stopped moving.
    pub fn is_terminal(self) -> bool {
        !matches!(self, OperationState::Pending | OperationState::Running)
    }
}

impl std::fmt::Display for OperationState {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match serde_json::to_value(self) {
            Ok(serde_json::Value::String(name)) => formatter.write_str(&name),
            other => unreachable!("a unit state serializes as its name, not {other:?}"),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct OperationV1 {
    pub id: String,
    pub state: OperationState,
    #[serde(default)]
    pub cursor: Option<String>,
    #[serde(default)]
    pub events: Vec<OperationEvent>,
    #[serde(default)]
    pub expires_at_ms: Option<u64>,
}

impl OperationV1 {
    /// Refuses an operation whose stated expiry has passed. The expiry is
    /// read on the clock; a clock that cannot be read is refused by name,
    /// never read as time zero, which put every expiry still ahead.
    pub(crate) fn refuse_expired(&self) -> Result<(), WelesError> {
        let Some(expiry) = self.expires_at_ms else {
            return Ok(());
        };
        let now =
            crate::clock::unix_millis().map_err(|error| WelesError::Clock(error.to_string()))?;
        if now >= expiry {
            return Err(WelesError::ExpiredOperation);
        }
        Ok(())
    }
}

pub trait InteractionBridge {
    fn elicit(&self, prompt: &str, options: &[String], secret: bool) -> Result<String, String>;
    fn event(&self, event: &OperationEvent);
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WelesError {
    Unconfigured,
    Transport(String),
    Http {
        status: u16,
        message: String,
    },
    InvalidResponse(String),
    InvalidRequest(&'static str),
    UnknownProvider(String),
    UnavailableProvider {
        provider: String,
        reason: String,
    },
    Cancelled,
    RateLimited {
        retry_after_ms: Option<u64>,
    },
    ExpiredOperation,
    /// The feed answered a holding read with no new event and no end: the
    /// deployment does not hold a read, and this client never polls.
    NoHoldingRead,
    Operation {
        code: String,
        message: String,
    },
    Interaction(String),
    /// The host clock gave no time to judge an operation's expiry against.
    Clock(String),
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
            Self::NoHoldingRead => f.write_str(
                "Weles answered a holding operation read before the operation moved; the deployment has no holding read, and Jeden does not poll on a timer",
            ),
            Self::Operation { code, message } => {
                write!(f, "Weles operation failed ({code}): {message}")
            }
            Self::Interaction(e) => write!(f, "Weles interaction failed: {e}"),
            Self::Clock(e) => write!(f, "the expiry of a Weles operation cannot be judged: {e}"),
        }
    }
}
