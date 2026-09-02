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
        emit: Arc<dyn Fn(String, Value, bool) + Send + Sync>,
    ) -> Result<Value, String>;
    fn abort(&self, tenant: &TenantId, session_id: &str, request_id: &str) -> Result<bool, String>;
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

impl SessionBackend for AgentSessionFacade {
    fn create(&self, tenant: &TenantId, session_id: &str) -> Result<PathBuf, String> {
        let cwd = self
            .tenant_guard
            .tenant_root(tenant)
            .join("workspaces")
            .join(session_id);
        std::fs::create_dir_all(&cwd).map_err(|error| error.to_string())?;
        let session = AgentSession::new(SessionOptions {
            cwd,
            ..SessionOptions::default()
        })?;
        let path = session.session_path()?;
        self.sessions
            .lock()
            .map_err(|_| "agent session lock poisoned".to_string())?
            .insert((tenant.as_str().to_owned(), session_id.to_owned()), session);
        Ok(path)
    }

    fn open(&self, tenant: &TenantId, session_id: &str, dir: &Path) -> Result<PathBuf, String> {
        // `AgentSession::resume_in_place` (rust/sdk/session.rs:135) replays the
        // ledger at `dir` and keeps its recorder pointed at that same directory,
        // so every later turn appends to the ledger the operator's own terminal
        // wrote. Plain `AgentSession::resume` (rust/sdk/session.rs:112) would
        // seed a fresh session directory instead, which is a fork, not a
        // continuation. The session's own recorded `cwd` is reused as the
        // working directory so tools resolve exactly as they did on the host.
        let session = AgentSession::resume_in_place(
            SessionOptions {
                cwd: session_cwd(dir)?,
                ..SessionOptions::default()
            },
            dir,
        )?;
        let path = session.session_path()?;
        self.sessions
            .lock()
            .map_err(|_| "agent session lock poisoned".to_string())?
            .insert((tenant.as_str().to_owned(), session_id.to_owned()), session);
        Ok(path)
    }

    fn turns(&self, dir: &Path) -> Result<Vec<Value>, String> {
        crate::cli::sessions::session_conversation_turns(dir)
    }

    fn prompt(
        &self,
        tenant: &TenantId,
        session_id: &str,
        request_id: &str,
        prompt: &str,
        emit: Arc<dyn Fn(String, Value, bool) + Send + Sync>,
    ) -> Result<Value, String> {
        let session = self.session(tenant, session_id)?;
        let subscription = session.subscribe()?;
        let forwarding_request = request_id.to_owned();
        let forward_emit = emit.clone();
        let forwarder = thread::spawn(move || loop {
            match subscription.recv_timeout(Duration::from_millis(250)) {
                Ok(event) if event.request_id == forwarding_request => {
                    let (kind, payload, terminal) = map_event(event.event);
                    forward_emit(kind, payload, terminal);
                    if terminal {
                        break;
                    }
                }
                Ok(_) => continue,
                Err(std::sync::mpsc::RecvTimeoutError::Timeout) => continue,
                Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => break,
            }
        });
        let result = session.prompt(PromptRequest {
            request_id: request_id.to_owned(),
            prompt: prompt.to_owned(),
            goal: None,
        });
        if let Err(error) = &result {
            emit("error".into(), json!({"message": error}), true);
        }
        forwarder
            .join()
            .map_err(|_| "session event forwarder panicked".to_string())?;
        result.map(|result| {
            serde_json::to_value(result).unwrap_or_else(|_| json!({"requestId": request_id}))
        })
    }

    fn abort(&self, tenant: &TenantId, session_id: &str, request_id: &str) -> Result<bool, String> {
        self.session(tenant, session_id)?.abort(request_id)
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

impl<B: SessionBackend> SessionService<B> {
    pub fn new(
        backend: Arc<B>,
        tenants: TenantGuard,
        idempotency: IdempotencyStore,
        replay: ReplayStore,
        executor: Arc<BoundedExecutor>,
    ) -> Self {
        Self {
            backend,
            tenants,
            idempotency,
            replay,
            executor,
            sessions: Mutex::new(HashMap::new()),
            instance: instance_stamp(),
            next_session: AtomicU64::new(1),
            next_request: AtomicU64::new(1),
        }
    }

    pub fn create_session(&self, caller: &TenantPrincipal) -> Result<String, ServiceError> {
        self.tenants.register_session(&caller.tenant)?;
        let id = format!(
            "session-{}-{}",
            self.instance,
            self.next_session.fetch_add(1, Ordering::Relaxed)
        );
        let path = match self.backend.create(&caller.tenant, &id) {
            Ok(path) => path,
            Err(error) => {
                let _ = self.tenants.release_session(&caller.tenant);
                return Err(ServiceError::Runtime(error));
            }
        };
        self.sessions
            .lock()
            .map_err(|_| ServiceError::Runtime("session registry lock poisoned".into()))?
            .insert(
                id.clone(),
                SessionEntry {
                    tenant: caller.tenant.clone(),
                    path,
                },
            );
        Ok(id)
    }

    /// Every session the caller may see, newest first. With granted workspaces
    /// that is the host's own ledgers under `session_root()`; without one it is
    /// exactly what this tenant created through this daemon.
    pub fn list_sessions(
        &self,
        caller: &TenantPrincipal,
        limit: Option<usize>,
    ) -> Result<SessionListing, ServiceError> {
        let held = self.held_sessions()?;
        let mut sessions = Vec::new();
        let mut skipped = 0usize;
        if caller.workspaces().is_empty() {
            for (id, entry) in held.iter() {
                if entry.tenant != caller.tenant {
                    continue;
                }
                match self.summarize(id, &entry.path, true) {
                    Ok(summary) => sessions.push(summary),
                    Err(()) => skipped += 1,
                }
            }
        } else {
            let root = crate::session_root();
            let entries = std::fs::read_dir(&root).map_err(|error| {
                ServiceError::Runtime(format!("cannot read {}: {}", root.display(), error))
            })?;
            for entry in entries.flatten() {
                let dir = entry.path();
                if !dir.is_dir() {
                    continue;
                }
                let id = entry.file_name().to_string_lossy().into_owned();
                let cwd = match session_state(&dir) {
                    Ok(state) => state.cwd,
                    Err(_) => {
                        skipped += 1;
                        continue;
                    }
                };
                if !caller.grants_path(&cwd) {
                    continue;
                }
                match self.summarize(&id, &dir, held.contains_key(&id)) {
                    Ok(summary) => sessions.push(summary),
                    Err(()) => skipped += 1,
                }
            }
        }
        // Newest first; `startedAt` is a string of epoch seconds, so it is
        // compared numerically and anything unparseable sorts last.
        sessions.sort_by(|left, right| {
            let rank = |value: &str| value.parse::<u64>().ok();
            match (rank(&right.started_at), rank(&left.started_at)) {
                (Some(right), Some(left)) => right.cmp(&left),
                (Some(_), None) => std::cmp::Ordering::Greater,
                (None, Some(_)) => std::cmp::Ordering::Less,
                (None, None) => std::cmp::Ordering::Equal,
            }
        });
        if let Some(limit) = limit {
            sessions.truncate(limit);
        }
        Ok(SessionListing { sessions, skipped })
    }

    /// Resume one of the host's own sessions and register it under its own host
    /// session id, so `session/prompt`, `session/replay` and `session/cancel`
    /// address it exactly like a created session.
    pub fn open_session(
        &self,
        caller: &TenantPrincipal,
        session_id: &str,
    ) -> Result<SessionSummary, ServiceError> {
        let dir = self.resolve_granted_session(caller, session_id)?;
        if let Some(entry) = self.held_sessions()?.get(session_id) {
            if entry.tenant != caller.tenant {
                return Err(ServiceError::AccessDenied);
            }
            // Opening twice is idempotent: no second backend session, no second
            // quota unit.
            return self
                .summarize(session_id, &entry.path, true)
                .map_err(|()| unreadable_session(session_id));
        }
        self.tenants.register_session(&caller.tenant)?;
        let path = match self.backend.open(&caller.tenant, session_id, &dir) {
            Ok(path) => path,
            Err(error) => {
                let _ = self.tenants.release_session(&caller.tenant);
                return Err(ServiceError::Runtime(error));
            }
        };
        let summary = match self.summarize(session_id, &path, true) {
            Ok(summary) => summary,
            Err(()) => {
                let _ = self.tenants.release_session(&caller.tenant);
                return Err(unreadable_session(session_id));
            }
        };
        // Registered under the HOST session id, in the very map
        // `authorize_session` consults, so a following `session/prompt` reaches
        // the ledger the terminal wrote: the backend session behind this id was
        // resumed in place by `AgentSession::resume_in_place`
        // (rust/sdk/session.rs:135) and keeps appending to `path`.
        self.sessions
            .lock()
            .map_err(|_| ServiceError::Runtime("session registry lock poisoned".into()))?
            .insert(
                session_id.to_owned(),
                SessionEntry {
                    tenant: caller.tenant.clone(),
                    path,
                },
            );
        Ok(summary)
    }

    /// The replayed turns of a session the caller may open or already created.
    /// Read-only: it neither resumes nor registers the session. `limit` keeps
    /// the newest N turns and reports whether anything older was dropped.
    pub fn history(
        &self,
        caller: &TenantPrincipal,
        session_id: &str,
        limit: Option<usize>,
    ) -> Result<(Vec<Value>, bool), ServiceError> {
        let dir = match self.held_sessions()?.get(session_id) {
            Some(entry) if entry.tenant == caller.tenant => entry.path.clone(),
            Some(_) => return Err(ServiceError::AccessDenied),
            None => self.resolve_granted_session(caller, session_id)?,
        };
        let mut turns = self
            .backend
            .turns(&dir)
            .map_err(|_| unreadable_session(session_id))?;
        let truncated = limit.is_some_and(|limit| turns.len() > limit);
        if let Some(limit) = limit {
            if truncated {
                let dropped = turns.len() - limit;
                turns.drain(..dropped);
            }
        }
        Ok((turns, truncated))
    }

    fn held_sessions(&self) -> Result<HashMap<String, SessionEntry>, ServiceError> {
        self.sessions
            .lock()
            .map_err(|_| ServiceError::Runtime("session registry lock poisoned".into()))
            .map(|sessions| sessions.clone())
    }

    /// `Err(())` means the ledger could not be read; callers either skip the row
    /// or turn it into a typed refusal.
    fn summarize(&self, session_id: &str, dir: &Path, open: bool) -> Result<SessionSummary, ()> {
        let state = session_state(dir).map_err(|_| ())?;
        let turns = self.backend.turns(dir).map_err(|_| ())?;
        Ok(SessionSummary {
            session_id: session_id.to_owned(),
            path: dir.to_path_buf(),
            cwd: state.cwd,
            started_at: state.started_at,
            turns: turns.len(),
            open,
        })
    }

    /// A session id is only a host session when it names one directory directly
    /// under `session_root()` whose recorded `cwd` sits in a granted workspace.
    fn resolve_granted_session(
        &self,
        caller: &TenantPrincipal,
        session_id: &str,
    ) -> Result<PathBuf, ServiceError> {
        if session_id.trim().is_empty() {
            return Err(ServiceError::InvalidRequest(
                "sessionId must not be empty".into(),
            ));
        }
        if caller.workspaces().is_empty() {
            return Err(ServiceError::AccessDenied);
        }
        let candidate = PathBuf::from(session_id);
        let mut components = candidate.components();
        if !matches!(components.next(), Some(std::path::Component::Normal(_)))
            || components.next().is_some()
        {
            return Err(ServiceError::AccessDenied);
        }
        let dir = crate::session_root().join(session_id);
        if !dir.join("state.json").is_file() {
            return Err(ServiceError::AccessDenied);
        }
        let state = session_state(&dir).map_err(|_| ServiceError::AccessDenied)?;
        if !caller.grants_path(&state.cwd) {
            return Err(ServiceError::AccessDenied);
        }
        Ok(dir)
    }

    pub fn submit_prompt(
        self: &Arc<Self>,
        caller: &TenantPrincipal,
        session_id: &str,
        idempotency_key: &str,
        prompt: &str,
    ) -> Result<SubmitOutcome, ServiceError> {
        if prompt.trim().is_empty() {
            return Err(ServiceError::InvalidRequest(
                "prompt must not be empty".into(),
            ));
        }
        self.authorize_session(caller, session_id)?;
        let request_digest = IdempotencyStore::request_digest(prompt.as_bytes());
        let request_id = format!(
            "request-{}-{}",
            self.instance,
            self.next_request.fetch_add(1, Ordering::Relaxed)
        );
        match self.idempotency.begin(
            &caller.tenant,
            idempotency_key,
            &request_digest,
            &request_id,
        )? {
            IdempotencyDecision::Reattach { request_id } => {
                return Ok(SubmitOutcome::Reattached { request_id })
            }
            IdempotencyDecision::Completed { request_id, result } => {
                return Ok(SubmitOutcome::Completed { request_id, result })
            }
            IdempotencyDecision::Start => {}
        }
        let permit = match self.tenants.reserve_request(&caller.tenant) {
            Ok(permit) => permit,
            Err(error) => {
                self.idempotency.abandon(
                    &caller.tenant,
                    idempotency_key,
                    &request_digest,
                    &request_id,
                )?;
                return Err(error.into());
            }
        };
        let service = self.clone();
        let tenant = caller.tenant.clone();
        let session_id = session_id.to_owned();
        let key = idempotency_key.to_owned();
        let prompt = prompt.to_owned();
        let returned_request_id = request_id.clone();
        let rollback_tenant = tenant.clone();
        let rollback_key = key.clone();
        let rollback_digest = request_digest.clone();
        let rollback_request = request_id.clone();
        let submission = self.executor.submit(move || {
            let _permit = permit;
            let stream_id = request_id.clone();
            let replay = service.replay.clone();
            let event_tenant = tenant.clone();
            let event_session = session_id.clone();
            let event_request = request_id.clone();
            let emit = Arc::new(move |kind: String, payload: Value, terminal: bool| {
                let _ = replay.append(
                    &event_tenant,
                    SessionEventV1 {
                        session_id: event_session.clone(),
                        stream_id: stream_id.clone(),
                        sequence: 0,
                        event_id: String::new(),
                        request_id: event_request.clone(),
                        kind,
                        payload,
                        terminal,
                    },
                );
            });
            let result =
                service
                    .backend
                    .prompt(&tenant, &session_id, &request_id, &prompt, emit.clone());
            let cached = match result {
                Ok(value) => value,
                Err(message) => json!({"error": {"code": "runtime_error", "message": message}}),
            };
            let _ = service
                .idempotency
                .complete(&tenant, &key, &request_digest, cached);
        });
        if let Err(error) = submission {
            self.idempotency.abandon(
                &rollback_tenant,
                &rollback_key,
                &rollback_digest,
                &rollback_request,
            )?;
            return Err(match error {
                SubmitError::NotReady => ServiceError::NotReady,
                SubmitError::Backpressure { retry_after_millis } => {
                    ServiceError::Backpressure { retry_after_millis }
                }
            });
        }
        Ok(SubmitOutcome::Started {
            request_id: returned_request_id,
        })
    }

    pub fn replay(
        &self,
        caller: &TenantPrincipal,
        session_id: &str,
        request_id: &str,
        after: EventCursor,
        limit: usize,
    ) -> Result<Vec<SessionEventV1>, ServiceError> {
        self.authorize_session(caller, session_id)?;
        self.replay
            .replay(&caller.tenant, session_id, request_id, after, limit)
            .map_err(Into::into)
    }

    pub fn replay_from_token(
        &self,
        caller: &TenantPrincipal,
        session_id: &str,
        request_id: &str,
        cursor: Option<&str>,
        limit: usize,
    ) -> Result<Vec<Value>, ServiceError> {
        let cursor = match cursor {
            Some(token) => EventCursor::parse(token)?,
            None => EventCursor(0),
        };
        self.replay(caller, session_id, request_id, cursor, limit)
            .map(|events| events.into_iter().map(|event| event.wire_value()).collect())
    }

    pub fn cancel(
        &self,
        caller: &TenantPrincipal,
        session_id: &str,
        request_id: &str,
    ) -> Result<bool, ServiceError> {
        self.authorize_session(caller, session_id)?;
        self.backend
            .abort(&caller.tenant, session_id, request_id)
            .map_err(ServiceError::Runtime)
    }

    fn authorize_session(
        &self,
        caller: &TenantPrincipal,
        session_id: &str,
    ) -> Result<(), ServiceError> {
        let sessions = self
            .sessions
            .lock()
            .map_err(|_| ServiceError::Runtime("session registry lock poisoned".into()))?;
        // An opened host session is registered in the same map under the same
        // host session id, so it authorizes exactly like a created one.
        match sessions.get(session_id) {
            Some(entry) if entry.tenant == caller.tenant => Ok(()),
            _ => Err(ServiceError::AccessDenied),
        }
    }

    pub fn artifact_path(
        &self,
        caller: &TenantPrincipal,
        session_id: &str,
        relative: &Path,
    ) -> Result<PathBuf, ServiceError> {
        self.authorize_session(caller, session_id)?;
        self.tenants
            .scoped_path(
                &caller.tenant,
                &PathBuf::from("artifacts").join(session_id).join(relative),
            )
            .map_err(Into::into)
    }

    pub fn readiness(&self) -> Readiness {
        self.executor.readiness()
    }

    pub fn drain(&self, timeout: Duration) -> Result<(), ServiceError> {
        self.executor.drain(timeout).map_err(ServiceError::Runtime)
    }
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
        SessionEventKind::Result { text } => ("result".into(), json!({"text": text}), true),
        SessionEventKind::Error { message } => ("error".into(), json!({"message": message}), true),
    }
}
