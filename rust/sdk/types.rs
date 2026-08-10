use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::path::PathBuf;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct SessionOptions {
    pub cwd: PathBuf,
    #[serde(default)]
    pub model: Option<String>,
    #[serde(default)]
    pub max_tokens: Option<u32>,
    #[serde(default)]
    pub max_steps: Option<u32>,
    #[serde(default)]
    pub allow_write: bool,
    #[serde(default)]
    pub allow_command: bool,
    #[serde(default)]
    pub auto_approve: bool,
}

impl Default for SessionOptions {
    fn default() -> Self {
        Self {
            cwd: std::env::current_dir().unwrap_or_default(),
            model: None,
            max_tokens: None,
            max_steps: None,
            allow_write: false,
            allow_command: false,
            auto_approve: false,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PromptRequest {
    pub request_id: String,
    pub prompt: String,
    /// Exact objective for this turn. When present, Jeden keeps the model's work
    /// aligned to it and records it separately from the effective prompt.
    #[serde(default)]
    pub goal: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PromptResult {
    pub request_id: String,
    pub text: String,
    pub session_path: PathBuf,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum SessionEventKind {
    Status {
        message: String,
    },
    TextDelta {
        text: String,
    },
    Elicitation {
        token: String,
        question: String,
        options: Vec<String>,
    },
    Approval {
        token: String,
        tool: String,
        detail: String,
    },
    Result {
        text: String,
    },
    Goal {
        text: String,
    },
    Error {
        message: String,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionEvent {
    pub request_id: String,
    #[serde(flatten)]
    pub event: SessionEventKind,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Capabilities {
    pub protocol_version: u32,
    pub prompt: bool,
    pub abort: bool,
    pub resume: bool,
    pub event_subscription: bool,
    pub elicitation: bool,
    pub approval: bool,
    pub transports: Vec<String>,
}

impl Capabilities {
    pub(crate) fn current() -> Self {
        Self {
            protocol_version: 1,
            prompt: true,
            abort: true,
            resume: true,
            event_subscription: true,
            elicitation: true,
            approval: true,
            transports: vec!["ndjson".into(), "acp".into()],
        }
    }
}

#[derive(Debug, Clone)]
pub struct ElicitationRequest {
    pub token: String,
    pub request_id: String,
    pub question: String,
    pub options: Vec<String>,
}

#[derive(Debug, Clone)]
pub struct ApprovalRequest {
    pub token: String,
    pub request_id: String,
    pub tool: String,
    pub detail: String,
}

pub trait InteractionHandler: Send + Sync {
    fn elicit(&self, request: ElicitationRequest) -> Result<String, String>;
    fn approve(&self, request: ApprovalRequest) -> Result<bool, String>;
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RpcErrorData {
    pub code: String,
    pub details: Value,
}
