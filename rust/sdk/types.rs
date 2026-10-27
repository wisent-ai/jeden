use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::path::PathBuf;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", try_from = "StatedSessionOptions")]
pub struct SessionOptions {
    pub cwd: PathBuf,
    pub model: Option<String>,
    pub max_tokens: Option<u32>,
    pub max_steps: Option<u32>,
    pub allow_write: bool,
    pub allow_command: bool,
    pub auto_approve: bool,
}

impl SessionOptions {
    /// A session working in `cwd` with no model override, no limits beyond the
    /// configured ones, and no write, command or auto-approval grant.
    pub fn at(cwd: PathBuf) -> Self {
        Self {
            cwd,
            model: None,
            max_tokens: None,
            max_steps: None,
            allow_write: false,
            allow_command: false,
            auto_approve: false,
        }
    }
}

/// Session options as a client states them. An unstated `cwd` is the
/// directory the serving process runs in, the one its client started it in;
/// when that directory cannot be read the options are refused with its cause
/// rather than handed an empty path.
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct StatedSessionOptions {
    cwd: Option<PathBuf>,
    #[serde(default)]
    model: Option<String>,
    #[serde(default)]
    max_tokens: Option<u32>,
    #[serde(default)]
    max_steps: Option<u32>,
    #[serde(default)]
    allow_write: bool,
    #[serde(default)]
    allow_command: bool,
    #[serde(default)]
    auto_approve: bool,
}

impl TryFrom<StatedSessionOptions> for SessionOptions {
    type Error = String;

    fn try_from(stated: StatedSessionOptions) -> Result<Self, Self::Error> {
        let cwd = match stated.cwd {
            Some(cwd) => cwd,
            None => std::env::current_dir().map_err(|error| {
                format!("the session states no cwd and this process's working directory cannot be read ({error}); state options.cwd")
            })?,
        };
        Ok(Self {
            cwd,
            model: stated.model,
            max_tokens: stated.max_tokens,
            max_steps: stated.max_steps,
            allow_write: stated.allow_write,
            allow_command: stated.allow_command,
            auto_approve: stated.auto_approve,
        })
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
    pub completion: Value,
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
    /// A model reasoning chunk. Emitted only when the session's communication
    /// mode shows reasoning; never part of the answer.
    ReasoningDelta {
        text: String,
    },
    /// A tool the model invoked, with its input. Emitted only when the
    /// session's communication mode shows tool calls with their inputs.
    ToolCall {
        tool: String,
        input: Value,
    },
    /// What a tool returned, as recorded in the transcript. Emitted only when
    /// the session's communication mode shows tool results.
    ToolResult {
        tool: String,
        result: Value,
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
    /// Goal-lifecycle update from Oko's classifier: `status` is "active"
    /// (goal started) or "done" (goal finished).
    Goal {
        text: String,
        status: String,
    },
    Completion {
        state: Value,
    },
    AssistantMessage {
        text: String,
    },
    Result {
        text: String,
        completion: Value,
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
    /// The session protocol this runtime speaks, `meta.protocolVersion` on the wire.
    pub protocol_version: String,
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
            protocol_version: super::protocol::PROTOCOL_VERSION.to_owned(),
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
