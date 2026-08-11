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
        if result.is_err() {
            emit(
                "error".into(),
                json!({"message": result.as_ref().unwrap_err()}),
                true,
            );
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

pub struct SessionService<B: SessionBackend> {
    backend: Arc<B>,
    tenants: TenantGuard,
    idempotency: IdempotencyStore,
    replay: ReplayStore,
    executor: Arc<BoundedExecutor>,
    sessions: Mutex<HashMap<String, TenantId>>,
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
            next_session: AtomicU64::new(1),
            next_request: AtomicU64::new(1),
        }
    }

    pub fn create_session(&self, caller: &TenantPrincipal) -> Result<String, ServiceError> {
        self.tenants.register_session(&caller.tenant)?;
        let id = format!(
            "session-{}",
            self.next_session.fetch_add(1, Ordering::Relaxed)
        );
        if let Err(error) = self.backend.create(&caller.tenant, &id) {
            let _ = self.tenants.release_session(&caller.tenant);
            return Err(ServiceError::Runtime(error));
        }
        self.sessions
            .lock()
            .map_err(|_| ServiceError::Runtime("session registry lock poisoned".into()))?
            .insert(id.clone(), caller.tenant.clone());
        Ok(id)
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
            "request-{}",
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
        match sessions.get(session_id) {
            Some(owner) if owner == &caller.tenant => Ok(()),
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
        SessionEventKind::Result { text } => ("result".into(), json!({"text": text}), true),
        SessionEventKind::Error { message } => ("error".into(), json!({"message": message}), true),
    }
}
