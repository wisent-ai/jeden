use super::service::{ServiceError, SessionBackend, SessionService, SubmitOutcome};
use super::tenant::{TenantDirectory, TenantError};
use super::tls::ReloadableTlsAcceptor;
use super::transport::{AuthenticatedConnection, ErrorV1, ReconnectTokens, RequestEnvelopeV1};
use serde_json::{json, Value};
use std::net::SocketAddr;
use std::time::{SystemTime, UNIX_EPOCH};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::{watch, Semaphore};
use tokio::task::JoinSet;

use std::sync::atomic::{AtomicU8, AtomicUsize, Ordering};
use std::sync::{mpsc, Arc, Condvar, Mutex};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

type Job = Box<dyn FnOnce() + Send + 'static>;
const STARTING: u8 = 0;
const READY: u8 = 1;
const DRAINING: u8 = 2;
const STOPPED: u8 = 3;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Readiness {
    Starting,
    Ready,
    Draining,
    Stopped,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SubmitError {
    NotReady,
    Backpressure { retry_after_millis: u64 },
}

struct ExecutorInner {
    state: AtomicU8,
    queued_and_running: AtomicUsize,
    sender: Mutex<Option<mpsc::SyncSender<Job>>>,
    idle: Condvar,
    idle_lock: Mutex<()>,
}

pub struct BoundedExecutor {
    inner: Arc<ExecutorInner>,
    workers: Mutex<Vec<JoinHandle<()>>>,
}

impl BoundedExecutor {
    pub fn new(worker_count: usize, queue_capacity: usize) -> Result<Self, String> {
        if worker_count == 0 || queue_capacity == 0 {
            return Err("worker_count and queue_capacity must be non-zero".into());
        }
        let (sender, receiver) = mpsc::sync_channel::<Job>(queue_capacity);
        let receiver = Arc::new(Mutex::new(receiver));
        let inner = Arc::new(ExecutorInner {
            state: AtomicU8::new(STARTING),
            queued_and_running: AtomicUsize::new(0),
            sender: Mutex::new(Some(sender)),
            idle: Condvar::new(),
            idle_lock: Mutex::new(()),
        });
        let mut workers = Vec::with_capacity(worker_count);
        for index in 0..worker_count {
            let receiver = receiver.clone();
            let worker_inner = inner.clone();
            workers.push(
                thread::Builder::new()
                    .name(format!("jeden-session-{}", index))
                    .spawn(move || loop {
                        let job = match receiver.lock() {
                            Ok(receiver) => receiver.recv(),
                            Err(_) => break,
                        };
                        match job {
                            Ok(job) => {
                                job();
                                if worker_inner
                                    .queued_and_running
                                    .fetch_sub(1, Ordering::AcqRel)
                                    == 1
                                {
                                    worker_inner.idle.notify_all();
                                }
                            }
                            Err(_) => break,
                        }
                    })
                    .map_err(|error| error.to_string())?,
            );
        }
        inner.state.store(READY, Ordering::Release);
        Ok(Self {
            inner,
            workers: Mutex::new(workers),
        })
    }

    pub fn readiness(&self) -> Readiness {
        match self.inner.state.load(Ordering::Acquire) {
            STARTING => Readiness::Starting,
            READY => Readiness::Ready,
            DRAINING => Readiness::Draining,
            _ => Readiness::Stopped,
        }
    }

    pub fn submit(&self, job: impl FnOnce() + Send + 'static) -> Result<(), SubmitError> {
        if self.inner.state.load(Ordering::Acquire) != READY {
            return Err(SubmitError::NotReady);
        }
        self.inner.queued_and_running.fetch_add(1, Ordering::AcqRel);
        let result = self
            .inner
            .sender
            .lock()
            .ok()
            .and_then(|sender| sender.as_ref().cloned())
            .ok_or(SubmitError::NotReady)
            .and_then(|sender| {
                sender.try_send(Box::new(job)).map_err(|error| match error {
                    mpsc::TrySendError::Full(_) => SubmitError::Backpressure {
                        retry_after_millis: 100,
                    },
                    mpsc::TrySendError::Disconnected(_) => SubmitError::NotReady,
                })
            });
        if result.is_err() {
            self.inner.queued_and_running.fetch_sub(1, Ordering::AcqRel);
        }
        result
    }

    pub fn drain(&self, timeout: Duration) -> Result<(), String> {
        let prior =
            self.inner
                .state
                .compare_exchange(READY, DRAINING, Ordering::AcqRel, Ordering::Acquire);
        if prior.is_err() && self.inner.state.load(Ordering::Acquire) != DRAINING {
            return Ok(());
        }
        let deadline = Instant::now() + timeout;
        let mut guard = self
            .inner
            .idle_lock
            .lock()
            .map_err(|_| "executor idle lock poisoned")?;
        while self.inner.queued_and_running.load(Ordering::Acquire) != 0 {
            let remaining = deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                return Err("graceful drain timed out with active operations".into());
            }
            let waited = self
                .inner
                .idle
                .wait_timeout(guard, remaining)
                .map_err(|_| "executor idle lock poisoned")?;
            guard = waited.0;
        }
        self.inner
            .sender
            .lock()
            .map_err(|_| "executor sender lock poisoned")?
            .take();
        let workers = std::mem::take(
            &mut *self
                .workers
                .lock()
                .map_err(|_| "executor workers lock poisoned")?,
        );
        for worker in workers {
            worker.join().map_err(|_| "executor worker panicked")?;
        }
        self.inner.state.store(STOPPED, Ordering::Release);
        Ok(())
    }
}

impl Drop for BoundedExecutor {
    fn drop(&mut self) {
        self.inner.state.store(DRAINING, Ordering::Release);
        if let Ok(mut sender) = self.inner.sender.lock() {
            sender.take();
        }
        if let Ok(workers) = self.workers.get_mut() {
            for worker in workers.drain(..) {
                let _ = worker.join();
            }
        }
        self.inner.state.store(STOPPED, Ordering::Release);
    }
}

#[derive(Debug, Clone)]
pub struct HeadlessConfig {
    pub max_frame_bytes: usize,
    pub read_timeout: Duration,
    pub write_timeout: Duration,
    pub drain_timeout: Duration,
    pub max_connections: usize,
    pub reconnect_key: Vec<u8>,
    pub reconnect_ttl: Duration,
}

impl Default for HeadlessConfig {
    fn default() -> Self {
        Self {
            max_frame_bytes: 1024 * 1024,
            read_timeout: Duration::from_secs(30),
            write_timeout: Duration::from_secs(10),
            drain_timeout: Duration::from_secs(30),
            max_connections: 128,
            reconnect_key: vec![0; 32],
            reconnect_ttl: Duration::from_secs(300),
        }
    }
}

pub struct HeadlessDaemon<B: SessionBackend> {
    tls: ReloadableTlsAcceptor,
    directory: TenantDirectory,
    service: Arc<SessionService<B>>,
    config: HeadlessConfig,
    reconnect: ReconnectTokens,
}

impl<B: SessionBackend> HeadlessDaemon<B> {
    pub fn new(
        tls: ReloadableTlsAcceptor,
        directory: TenantDirectory,
        service: Arc<SessionService<B>>,
        config: HeadlessConfig,
    ) -> Result<Self, String> {
        if config.max_frame_bytes == 0 || config.max_connections == 0 {
            return Err("max_frame_bytes and max_connections must be non-zero".into());
        }
        let reconnect = ReconnectTokens::new(config.reconnect_key.clone())
            .map_err(|_| "reconnect key must contain at least 32 bytes")?;
        Ok(Self {
            tls,
            directory,
            service,
            config,
            reconnect,
        })
    }

    pub fn local_addr(listener: &TcpListener) -> Result<SocketAddr, String> {
        listener.local_addr().map_err(|error| error.to_string())
    }

    pub async fn serve(
        self: Arc<Self>,
        listener: TcpListener,
        mut shutdown: watch::Receiver<bool>,
    ) -> Result<(), String> {
        let admission = Arc::new(Semaphore::new(self.config.max_connections));
        let mut connections = JoinSet::new();
        loop {
            tokio::select! {
                changed = shutdown.changed() => {
                    if changed.is_err() || *shutdown.borrow() { break; }
                }
                accepted = listener.accept() => {
                    let (stream, _) = accepted.map_err(|error| format!("TCP accept failed: {error}"))?;
                    let daemon = self.clone();
                    let permit = admission.clone().try_acquire_owned();
                    connections.spawn(async move {
                        let _permit = match permit {
                            Ok(permit) => permit,
                            Err(_) => { reject_overloaded(stream, daemon.config.write_timeout).await; return; }
                        };
                        let _ = daemon.serve_connection(stream).await;
                    });
                }
            }
        }
        let service = self.service.clone();
        let drain_timeout = self.config.drain_timeout;
        tokio::task::spawn_blocking(move || service.drain(drain_timeout))
            .await
            .map_err(|error| error.to_string())?
            .map_err(|error| format!("graceful drain failed: {error:?}"))?;
        while connections.join_next().await.is_some() {}
        Ok(())
    }

    async fn serve_connection(&self, mut stream: TcpStream) -> Result<(), String> {
        let mut record_prefix = [0_u8; 3];
        tokio::time::timeout(self.config.read_timeout, async {
            loop {
                let prefix_len = stream
                    .peek(&mut record_prefix)
                    .await
                    .map_err(|error| format!("TLS preface read failed: {error}"))?;
                if prefix_len == 0 {
                    return Err("connection closed before TLS preface".to_string());
                }
                if prefix_len >= record_prefix.len() {
                    return Ok(());
                }
                tokio::task::yield_now().await;
            }
        })
        .await
        .map_err(|_| "TLS preface deadline exceeded".to_string())??;
        if record_prefix[0] != 0x16 || record_prefix[1] != 0x03 {
            let _ = stream.shutdown().await;
            return Err("plaintext or malformed TLS preface rejected".into());
        }
        let (stream, verified) =
            tokio::time::timeout(self.config.read_timeout, self.tls.accept(stream))
                .await
                .map_err(|_| "TLS handshake deadline exceeded".to_string())??;
        let identity = self
            .directory
            .resolve(&verified)
            .map_err(|_| "certificate SAN is not mapped".to_string())?;
        let connection = AuthenticatedConnection {
            identity,
            trust_generation: verified.trust_generation,
        };
        let (reader, mut writer) = tokio::io::split(stream);
        let mut reader = BufReader::new(reader);
        loop {
            let frame = match read_async_frame(
                &mut reader,
                self.config.max_frame_bytes,
                self.config.read_timeout,
            )
            .await
            {
                Ok(Some(frame)) => frame,
                Ok(None) => return Ok(()),
                Err(error) => {
                    let response = wire_error(
                        Value::Null,
                        ErrorV1 {
                            code: "malformed_frame".into(),
                            message: error,
                            retryable: false,
                            details: json!({}),
                        },
                    );
                    write_async_frame(&mut writer, &response, self.config.write_timeout).await?;
                    return Ok(());
                }
            };
            let request: RequestEnvelopeV1 = match serde_json::from_slice(&frame) {
                Ok(request) => request,
                Err(error) => {
                    let response = wire_error(
                        Value::Null,
                        ErrorV1 {
                            code: "malformed_json".into(),
                            message: error.to_string(),
                            retryable: false,
                            details: json!({}),
                        },
                    );
                    write_async_frame(&mut writer, &response, self.config.write_timeout).await?;
                    continue;
                }
            };
            let id = Value::String(request.id.clone());
            let response = match self.dispatch(&connection, request) {
                Ok(result) => json!({"id": id, "result": result}),
                Err(error) => wire_error(id, error),
            };
            write_async_frame(&mut writer, &response, self.config.write_timeout).await?;
        }
    }

    fn dispatch(
        &self,
        connection: &AuthenticatedConnection,
        request: RequestEnvelopeV1,
    ) -> Result<Value, ErrorV1> {
        connection.validate_request(&request).map_err(|_| {
            protocol_error(
                "unsupported_protocol",
                "protocolVersion must be jeden.session.v1",
            )
        })?;
        if let Some(deadline) = request.meta.deadline_unix_millis {
            if now_unix_millis() >= deadline {
                return Err(protocol_error(
                    "deadline_exceeded",
                    "request deadline has elapsed",
                ));
            }
        }
        match request.method.as_str() {
            "health/readiness" | "readiness" => {
                Ok(json!({"state": format!("{:?}", self.service.readiness()).to_lowercase()}))
            }
            "session/create" => {
                let session_id = self
                    .service
                    .create_session(&connection.identity)
                    .map_err(service_error)?;
                let expires = now_unix().saturating_add(self.config.reconnect_ttl.as_secs());
                let reconnect_token = self
                    .reconnect
                    .issue(connection, &session_id, expires)
                    .map_err(|_| protocol_error("internal", "failed to issue reconnect token"))?;
                Ok(
                    json!({"sessionId": session_id, "reconnectToken": reconnect_token, "expiresUnix": expires}),
                )
            }
            "session/reconnect" => {
                let token = string_field(&request.params, "reconnectToken")?;
                let session_id = self
                    .reconnect
                    .verify(connection, token, now_unix())
                    .map_err(|_| {
                        protocol_error("access_denied", "invalid or expired reconnect token")
                    })?;
                Ok(json!({"sessionId": session_id}))
            }
            "session/prompt" => {
                let session_id = string_field(&request.params, "sessionId")?;
                let prompt = string_field(&request.params, "prompt")?;
                let outcome = self
                    .service
                    .submit_prompt(
                        &connection.identity,
                        session_id,
                        &request.meta.idempotency_key,
                        prompt,
                    )
                    .map_err(service_error)?;
                Ok(match outcome {
                    SubmitOutcome::Started { request_id } => {
                        json!({"state": "started", "requestId": request_id})
                    }
                    SubmitOutcome::Reattached { request_id } => {
                        json!({"state": "reattached", "requestId": request_id})
                    }
                    SubmitOutcome::Completed { request_id, result } => {
                        json!({"state": "completed", "requestId": request_id, "result": result})
                    }
                })
            }
            "session/replay" => {
                let session_id = string_field(&request.params, "sessionId")?;
                let request_id = string_field(&request.params, "requestId")?;
                let cursor = request.params.get("cursor").and_then(Value::as_str);
                let limit = request
                    .params
                    .get("limit")
                    .and_then(Value::as_u64)
                    .unwrap_or(100)
                    .min(1000) as usize;
                let events = self
                    .service
                    .replay_from_token(&connection.identity, session_id, request_id, cursor, limit)
                    .map_err(service_error)?;
                Ok(json!({"events": events}))
            }
            "session/cancel" => {
                let session_id = string_field(&request.params, "sessionId")?;
                let request_id = string_field(&request.params, "requestId")?;
                let cancelled = self
                    .service
                    .cancel(&connection.identity, session_id, request_id)
                    .map_err(service_error)?;
                Ok(json!({"cancelled": cancelled}))
            }
            _ => Err(protocol_error("method_not_found", "unknown session method")),
        }
    }
}

async fn reject_overloaded(mut stream: TcpStream, timeout: Duration) {
    let response = wire_error(
        Value::Null,
        ErrorV1 {
            code: "backpressure".into(),
            message: "connection admission capacity exhausted".into(),
            retryable: true,
            details: json!({"retryAfterMillis": 100}),
        },
    );
    let _ = write_async_frame(&mut stream, &response, timeout).await;
    let _ = stream.shutdown().await;
}

async fn read_async_frame<R: tokio::io::AsyncBufRead + Unpin>(
    reader: &mut R,
    max: usize,
    deadline: Duration,
) -> Result<Option<Vec<u8>>, String> {
    let mut frame = Vec::new();
    let bytes = tokio::time::timeout(deadline, reader.read_until(b'\n', &mut frame))
        .await
        .map_err(|_| "frame read deadline exceeded".to_string())?
        .map_err(|error| error.to_string())?;
    if bytes == 0 {
        return Ok(None);
    }
    if frame.len() > max {
        return Err(format!("frame exceeds {max} bytes"));
    }
    while matches!(frame.last(), Some(b'\n' | b'\r')) {
        frame.pop();
    }
    if frame.is_empty() {
        return Err("empty frame".into());
    }
    Ok(Some(frame))
}

async fn write_async_frame<W: tokio::io::AsyncWrite + Unpin>(
    writer: &mut W,
    value: &Value,
    deadline: Duration,
) -> Result<(), String> {
    let mut encoded = serde_json::to_vec(value).map_err(|error| error.to_string())?;
    encoded.push(b'\n');
    tokio::time::timeout(deadline, writer.write_all(&encoded))
        .await
        .map_err(|_| "frame write deadline exceeded".to_string())?
        .map_err(|error| error.to_string())
}

fn string_field<'a>(params: &'a Value, field: &str) -> Result<&'a str, ErrorV1> {
    params
        .get(field)
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| protocol_error("invalid_request", &format!("missing {field}")))
}

fn wire_error(id: Value, error: ErrorV1) -> Value {
    json!({"id": id, "error": error})
}

fn protocol_error(code: &str, message: &str) -> ErrorV1 {
    ErrorV1 {
        code: code.into(),
        message: message.into(),
        retryable: false,
        details: json!({}),
    }
}

fn service_error(error: ServiceError) -> ErrorV1 {
    match error {
        ServiceError::AccessDenied => protocol_error("access_denied", "access denied"),
        ServiceError::InvalidRequest(message) => protocol_error("invalid_request", &message),
        ServiceError::Backpressure { retry_after_millis } => ErrorV1 {
            code: "backpressure".into(),
            message: "service capacity exhausted".into(),
            retryable: true,
            details: json!({"retryAfterMillis": retry_after_millis}),
        },
        ServiceError::NotReady => ErrorV1 {
            code: "not_ready".into(),
            message: "service is not ready".into(),
            retryable: true,
            details: json!({"retryAfterMillis": 100}),
        },
        ServiceError::Tenant(TenantError::QuotaExceeded { retry_after_millis }) => ErrorV1 {
            code: "quota_exceeded".into(),
            message: "tenant quota exceeded".into(),
            retryable: true,
            details: json!({"retryAfterMillis": retry_after_millis}),
        },
        ServiceError::Tenant(_) => protocol_error("access_denied", "access denied"),
        ServiceError::Idempotency(error) => {
            protocol_error("idempotency_error", &format!("{error:?}"))
        }
        ServiceError::Replay(error) => protocol_error("replay_error", &format!("{error:?}")),
        ServiceError::Runtime(message) => protocol_error("runtime_error", &message),
    }
}

fn now_unix() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}
fn now_unix_millis() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
        .min(u128::from(u64::MAX)) as u64
}
