mod backend;
mod registry;
mod execution;

use super::daemon::{BoundedExecutor, Readiness, SubmitError};
use super::idempotency::{IdempotencyDecision, IdempotencyError, IdempotencyStore};
use super::replay::{EventCursor, ReplayError, ReplayStore, SessionEventV1};
use super::tenant::{TenantError, TenantGuard, TenantId, TenantPrincipal};
use crate::sdk::{AgentSession, PromptRequest, SessionEventKind, SessionOptions};
use serde_json::{json, Value};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

#[derive(Debug, Clone, PartialEq)]
pub enum SubmitOutcome {
    Started { request_id: String },
    Reattached { request_id: String },
    Completed { request_id: String, result: Value },
}

#[derive(Debug, Clone, PartialEq)]
pub enum ServiceError {
    AccessDenied,
    InvalidRequest(String),
    Tenant(TenantError),
    Idempotency(IdempotencyError),
    Replay(ReplayError),
    Backpressure { retry_after_millis: u64 },
    NotReady,
    Runtime(String),
}

impl From<TenantError> for ServiceError {
    fn from(error: TenantError) -> Self {
        match error {
            TenantError::AccessDenied => Self::AccessDenied,
            other => Self::Tenant(other),
        }
    }
}
impl From<IdempotencyError> for ServiceError {
    fn from(error: IdempotencyError) -> Self {
        Self::Idempotency(error)
    }
}
impl From<ReplayError> for ServiceError {
    fn from(error: ReplayError) -> Self {
        Self::Replay(error)
    }
}

pub trait SessionBackend: Send + Sync + 'static {
    fn create(&self, tenant: &TenantId, session_id: &str) -> Result<PathBuf, String>;
    /// Resume an existing host session ledger at `dir` and register it under
    /// `session_id` so prompts address it exactly like a created session.
    fn open(&self, tenant: &TenantId, session_id: &str, dir: &Path) -> Result<PathBuf, String>;
    /// The replayed `{"role","content"}` turns of the ledger at `dir`.
    fn turns(&self, dir: &Path) -> Result<Vec<Value>, String>;
    fn prompt(
        &self,
        tenant: &TenantId,
        session_id: &str,
        request_id: &str,
        prompt: &str,
        continuing: bool,
        emit: Arc<dyn Fn(String, Value, bool) + Send + Sync>,
    ) -> Result<Value, String>;
    fn abort(&self, tenant: &TenantId, session_id: &str, request_id: &str) -> Result<bool, String>;
    fn completion(&self, tenant: &TenantId, session_id: &str) -> Result<Value, String>;
    fn add_request(&self, tenant: &TenantId, session_id: &str, prompt: &str) -> Result<Value, String>;
    fn control_completion(&self, tenant: &TenantId, session_id: &str, task_id: &str,
        action: &str, reason: &str, revision: u64) -> Result<Value, String>;
}

#[derive(Clone)]
pub struct AgentSessionFacade {
    tenant_guard: TenantGuard,
    sessions: Arc<Mutex<HashMap<(String, String), AgentSession>>>,
}

impl AgentSessionFacade {
    pub fn new(tenant_guard: TenantGuard) -> Self {
        Self {
            tenant_guard,
            sessions: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    fn session(&self, tenant: &TenantId, session_id: &str) -> Result<AgentSession, String> {
        self.sessions
            .lock()
            .map_err(|_| "agent session lock poisoned".to_string())?
            .get(&(tenant.as_str().to_owned(), session_id.to_owned()))
            .cloned()
            .ok_or_else(|| "access denied".to_string())
    }
}


/// One session this daemon holds: its owner and the ledger directory it writes.
#[derive(Debug, Clone)]
struct SessionEntry {
    tenant: TenantId,
    path: PathBuf,
}

/// One row of `session/list` and the reply of `session/open`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionSummary {
    pub session_id: String,
    pub path: PathBuf,
    pub cwd: PathBuf,
    pub started_at: String,
    pub turns: usize,
    pub open: bool,
}

impl SessionSummary {
    pub fn wire_value(&self) -> Value {
        json!({
            "sessionId": self.session_id,
            "path": self.path,
            "cwd": self.cwd,
            "startedAt": self.started_at,
            "turns": self.turns,
            "open": self.open,
        })
    }
}

/// `skipped` counts session directories whose `state.json` could not be read,
/// so an unreadable session is reported rather than silently dropped.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionListing {
    pub sessions: Vec<SessionSummary>,
    pub skipped: usize,
}

pub struct SessionService<B: SessionBackend> {
    backend: Arc<B>,
    tenants: TenantGuard,
    idempotency: IdempotencyStore,
    replay: ReplayStore,
    executor: Arc<BoundedExecutor>,
    sessions: Mutex<HashMap<String, SessionEntry>>,
    /// Both counters are process-local while the replay and idempotency stores on
    /// disk are durable, so a restarted daemon would hand out `session-1` and
    /// `request-1` again and its first prompt would land on a stream that already
    /// holds a terminal event — the append is refused and the client polls a stale,
    /// finished stream. The instance stamp is what makes an id unique for as long
    /// as those stores keep it.
    instance: String,
    next_session: AtomicU64,
    next_request: AtomicU64,
}


/// The two `state.json` fields a session row is built from.
struct SessionState {
    cwd: PathBuf,
    started_at: String,
}

/// A short, sortable stamp for one service instance: the second it started and a
/// random half, so two daemons started in the same second on one store still
/// hand out distinct ids.
fn instance_stamp() -> String {
    let started = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    let mut nonce = [0_u8; 3];
    rand::RngCore::fill_bytes(&mut rand::rngs::OsRng, &mut nonce);
    format!("{started}{}", hex::encode(nonce))
}

fn session_state(dir: &Path) -> Result<SessionState, String> {
    let path = dir.join("state.json");
    let value: Value = serde_json::from_slice(
        &std::fs::read(&path)
            .map_err(|error| format!("cannot read {}: {}", path.display(), error))?,
    )
    .map_err(|error| format!("invalid {}: {}", path.display(), error))?;
    let cwd = value
        .get("cwd")
        .and_then(Value::as_str)
        .ok_or_else(|| format!("{} has no cwd", path.display()))?;
    let started_at = value
        .get("startedAt")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_owned();
    Ok(SessionState {
        cwd: PathBuf::from(cwd),
        started_at,
    })
}

fn session_cwd(dir: &Path) -> Result<PathBuf, String> {
    session_state(dir).map(|state| state.cwd)
}

fn unreadable_session(session_id: &str) -> ServiceError {
    ServiceError::Runtime(format!("session {session_id} ledger is unreadable"))
}

fn map_event(event: SessionEventKind) -> (String, Value, bool) {
    match event {
        SessionEventKind::Status { message } => {
            ("status".into(), json!({"message": message}), false)
        }
        SessionEventKind::TextDelta { text } => ("textDelta".into(), json!({"text": text}), false),
        SessionEventKind::Completion { state } => ("completion".into(), json!({"state": state}), false),
        SessionEventKind::AssistantMessage { text } => ("assistantMessage".into(), json!({"text": text}), false),
        SessionEventKind::ReasoningDelta { text } => {
            ("reasoningDelta".into(), json!({"text": text}), false)
        }
        SessionEventKind::ToolCall { tool, input } => (
            "toolCall".into(),
            json!({"tool": tool, "input": input}),
            false,
        ),
        SessionEventKind::ToolResult { tool, result } => (
            "toolResult".into(),
            json!({"tool": tool, "result": result}),
            false,
        ),
        SessionEventKind::Elicitation {
            token,
            question,
            options,
        } => (
            "elicitation".into(),
            json!({"token": token, "question": question, "options": options}),
            false,
        ),
        SessionEventKind::Approval {
            token,
            tool,
            detail,
        } => (
            "approval".into(),
            json!({"token": token, "tool": tool, "detail": detail}),
            false,
        ),
        SessionEventKind::Goal { text, status } => (
            "goal".into(),
            json!({"text": text, "status": status}),
            false,
        ),
        SessionEventKind::Result { text, completion } => ("result".into(), json!({"text": text, "completion": completion}), true),
        SessionEventKind::Error { message } => ("error".into(), json!({"message": message}), true),
    }
}
