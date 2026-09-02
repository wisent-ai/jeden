use super::{
    AgentSessionFacade, BoundedExecutor, HeadlessConfig, HeadlessDaemon, IdempotencyStore,
    MtlsConfig, ReloadableTlsAcceptor, ReplayStore, SessionService, TenantDirectory, TenantError,
    TenantGuard, TenantLimits,
};
use rand::RngCore;
use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};

use super::interaction::RpcInteractionBridge;
use crate::sdk::{AgentSession, PromptRequest, SessionEventKind, SessionOptions};
use serde::Deserialize;
use serde_json::{json, Value};
use std::collections::HashMap;
use std::io::{self, BufRead, Write};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

const MAX_FRAME_BYTES: usize = 1024 * 1024;

#[derive(Clone)]
pub(crate) struct JsonWriter {
    inner: Arc<Mutex<Box<dyn Write + Send>>>,
}

impl JsonWriter {
    fn new<W: Write + Send + 'static>(writer: W) -> Self {
        Self {
            inner: Arc::new(Mutex::new(Box::new(writer))),
        }
    }

    pub(crate) fn send(&self, value: &Value) -> Result<(), String> {
        let mut writer = self.inner.lock().map_err(|_| "output lock poisoned")?;
        serde_json::to_writer(&mut *writer, value).map_err(|error| error.to_string())?;
        writer.write_all(b"\n").map_err(|error| error.to_string())?;
        writer.flush().map_err(|error| error.to_string())
    }
}

#[derive(Deserialize)]
struct WireRequest {
    #[serde(default)]
    id: Value,
    method: String,
    #[serde(default)]
    params: Value,
}

struct ServerState {
    sessions: Mutex<HashMap<String, AgentSession>>,
    bridge: Arc<RpcInteractionBridge>,
    writer: JsonWriter,
    next_session: AtomicU64,
    shutting_down: AtomicBool,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct HeadlessIdentityMapping {
    san: String,
    principal: String,
    tenant: String,
    /// Absolute host directories this principal may read and continue sessions
    /// inside. Absent means the principal only ever sees the scratch workspaces
    /// this daemon creates for it.
    #[serde(default)]
    workspaces: Vec<PathBuf>,
}

pub fn serve_headless_cli(positionals: &[String], data_root: &Path) -> Result<(), String> {
    if !(5..=6).contains(&positionals.len()) {
        return Err("Usage: jeden headless <addr> <server-cert.pem> <server-key.pem> <client-ca.pem> <identity-map.json> [revoked-serials.txt]".into());
    }
    let mappings: Vec<HeadlessIdentityMapping> = serde_json::from_slice(
        &fs::read(&positionals[4])
            .map_err(|error| format!("failed to read identity map: {error}"))?,
    )
    .map_err(|error| format!("invalid identity map: {error}"))?;
    if mappings.is_empty() {
        return Err("identity map must not be empty".into());
    }
    let directory = TenantDirectory::new();
    for mapping in mappings {
        let san = mapping.san.clone();
        directory
            .map_san(
                mapping.san,
                mapping.principal,
                mapping.tenant,
                mapping.workspaces,
            )
            .map_err(|error| match error {
                TenantError::InvalidWorkspace(message) => {
                    format!("invalid identity mapping for {san}: {message}")
                }
                _ => format!("invalid identity mapping for {san}"),
            })?;
    }
    let revoked_serials = positionals
        .get(5)
        .map(|path| read_revoked_serials(Path::new(path)))
        .transpose()?
        .unwrap_or_default();
    let tls = ReloadableTlsAcceptor::new(MtlsConfig {
        certificate_chain: PathBuf::from(&positionals[1]),
        private_key: PathBuf::from(&positionals[2]),
        client_ca_bundle: PathBuf::from(&positionals[3]),
        revoked_serials,
    })?;
    fs::create_dir_all(data_root)
        .map_err(|error| format!("failed to create headless data root: {error}"))?;
    let tenant_guard = TenantGuard::new(
        data_root.join("tenants"),
        TenantLimits {
            max_active_requests: 4,
            max_sessions: 32,
            max_stored_bytes: 1024 * 1024 * 1024,
        },
    );
    let backend = Arc::new(AgentSessionFacade::new(tenant_guard.clone()));
    let executor = Arc::new(BoundedExecutor::new(4, 64)?);
    let service = Arc::new(SessionService::new(
        backend,
        tenant_guard,
        IdempotencyStore::new(data_root.join("idempotency")),
        ReplayStore::new(data_root.join("replay"), 10_000),
        executor,
    ));
    let mut config = HeadlessConfig::default();
    config.reconnect_key = load_or_create_reconnect_key(&data_root.join("reconnect.key"))?;
    let daemon = Arc::new(HeadlessDaemon::new(tls, directory, service, config)?);
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .map_err(|error| error.to_string())?;
    runtime.block_on(async move {
        let listener = tokio::net::TcpListener::bind(&positionals[0])
            .await
            .map_err(|error| format!("failed to bind secure headless listener: {error}"))?;
        let (_shutdown_tx, shutdown_rx) = tokio::sync::watch::channel(false);
        daemon.serve(listener, shutdown_rx).await
    })
}

fn read_revoked_serials(path: &Path) -> Result<HashSet<String>, String> {
    let contents = fs::read_to_string(path)
        .map_err(|error| format!("failed to read revocation list: {error}"))?;
    Ok(contents
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty() && !line.starts_with('#'))
        .map(str::to_owned)
        .collect())
}

fn load_or_create_reconnect_key(path: &Path) -> Result<Vec<u8>, String> {
    match fs::read(path) {
        Ok(key) if key.len() >= 32 => return Ok(key),
        Ok(_) => return Err("stored reconnect key is shorter than 32 bytes".into()),
        Err(error) if error.kind() != io::ErrorKind::NotFound => {
            return Err(format!("failed to read reconnect key: {error}"))
        }
        Err(_) => {}
    }
    let mut key = vec![0_u8; 32];
    rand::rngs::OsRng.fill_bytes(&mut key);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        let mut options = fs::OpenOptions::new();
        options.write(true).create_new(true).mode(0o600);
        let mut file = options
            .open(path)
            .map_err(|error| format!("failed to create reconnect key: {error}"))?;
        file.write_all(&key)
            .map_err(|error| format!("failed to persist reconnect key: {error}"))?;
        file.sync_all()
            .map_err(|error| format!("failed to sync reconnect key: {error}"))?;
    }
    #[cfg(not(unix))]
    fs::write(path, &key).map_err(|error| format!("failed to persist reconnect key: {error}"))?;
    Ok(key)
}

pub fn serve_stdio() -> Result<(), String> {
    let input = io::BufReader::new(io::stdin());
    serve(input, io::stdout())
}

pub fn serve<R, W>(mut input: R, output: W) -> Result<(), String>
where
    R: BufRead,
    W: Write + Send + 'static,
{
    let writer = JsonWriter::new(output);
    let bridge = RpcInteractionBridge::new(writer.clone());
    let state = Arc::new(ServerState {
        sessions: Mutex::new(HashMap::new()),
        bridge,
        writer: writer.clone(),
        next_session: AtomicU64::new(1),
        shutting_down: AtomicBool::new(false),
    });
    writer.send(&json!({
        "type": "ready",
        "protocol": "jeden-rpc",
        "version": 1,
        "capabilities": AgentSession::capabilities()
    }))?;

    let mut workers = Vec::new();
    while !state.shutting_down.load(Ordering::Acquire) {
        let frame = match read_frame(&mut input) {
            Ok(Some(frame)) => frame,
            Ok(None) => break,
            Err(error) => {
                writer.send(&error_response(Value::Null, "malformed_frame", &error))?;
                continue;
            }
        };
        let request = match serde_json::from_slice::<WireRequest>(&frame) {
            Ok(request) => request,
            Err(error) => {
                writer.send(&error_response(
                    Value::Null,
                    "malformed_json",
                    &error.to_string(),
                ))?;
                continue;
            }
        };
        if request.method == "prompt" || request.method == "session/prompt" {
            let worker_state = state.clone();
            workers.push(thread::spawn(move || handle_prompt(worker_state, request)));
        } else if let Err(error) = handle_request(&state, request) {
            writer.send(&error)?;
        }
    }

    state.shutting_down.store(true, Ordering::Release);
    let sessions = state
        .sessions
        .lock()
        .map_err(|_| "sessions lock poisoned")?
        .drain()
        .map(|(_, session)| session)
        .collect::<Vec<_>>();
    for session in &sessions {
        let active = session.status()?;
        for request_id in active {
            let _ = session.abort(&request_id);
        }
    }
    state.bridge.cancel_all()?;
    for worker in workers {
        worker
            .join()
            .map_err(|_| "prompt worker panicked".to_string())??;
    }
    for session in sessions {
        session.dispose()?;
    }
    Ok(())
}

fn handle_request(state: &Arc<ServerState>, request: WireRequest) -> Result<(), Value> {
    let id = request.id.clone();
    let result = match request.method.as_str() {
        "initialize" | "capabilities" => Ok(json!({
            "protocol": "jeden-rpc",
            "capabilities": AgentSession::capabilities()
        })),
        "session/new" | "new" => create_session(state, request.params, false),
        "session/open" | "session/load" | "resume" => create_session(state, request.params, true),
        "abort" | "session/cancel" => abort_session(state, &request.params),
        "status" | "session/status" => session_status(state, &request.params),
        "dispose" | "session/dispose" => dispose_session(state, &request.params),
        "elicitation/resolve" | "session/input_response" => {
            resolve_elicitation(state, &request.params)
        }
        "approval/resolve" | "session/permission_response" => {
            resolve_approval(state, &request.params)
        }
        "shutdown" => {
            state.shutting_down.store(true, Ordering::Release);
            Ok(json!({"shuttingDown": true}))
        }
        _ => Err((
            "method_not_found",
            format!("unknown method: {}", request.method),
        )),
    };
    match result {
        Ok(value) => state
            .writer
            .send(&success_response(id, value))
            .map_err(|error| error_response(Value::Null, "write_error", &error)),
        Err((code, message)) => Err(error_response(id, code, &message)),
    }
}

fn handle_prompt(state: Arc<ServerState>, request: WireRequest) -> Result<(), String> {
    let id = request.id.clone();
    if let Err(error) = handle_prompt_inner(&state, request) {
        state
            .writer
            .send(&error_response(id, "prompt_failed", &error))?;
    }
    Ok(())
}

fn handle_prompt_inner(state: &Arc<ServerState>, request: WireRequest) -> Result<(), String> {
    let id = request.id.clone();
    let session_id = string_param(&request.params, "sessionId")?;
    let request_id = request
        .params
        .get("requestId")
        .and_then(Value::as_str)
        .map(str::to_string)
        .unwrap_or_else(|| wire_id(&id));
    let prompt = string_param(&request.params, "prompt")?;
    let goal = request
        .params
        .get("goal")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|goal| !goal.is_empty())
        .map(str::to_string);
    let session = state
        .sessions
        .lock()
        .map_err(|_| "sessions lock poisoned")?
        .get(&session_id)
        .cloned()
        .ok_or_else(|| format!("unknown session: {}", session_id))?;
    let subscription = session.subscribe()?;
    let event_writer = state.writer.clone();
    let event_request_id = request_id.clone();
    let prompt_done = Arc::new(AtomicBool::new(false));
    let forward_done = prompt_done.clone();
    let forwarder = thread::spawn(move || -> Result<(), String> {
        loop {
            match subscription.recv_timeout(Duration::from_secs(1)) {
                Ok(event) if event.request_id == event_request_id => {
                    let terminal = matches!(
                        &event.event,
                        SessionEventKind::Result { .. } | SessionEventKind::Error { .. }
                    );
                    event_writer.send(&json!({"method": "session/event", "params": event}))?;
                    if terminal {
                        return Ok(());
                    }
                }
                Ok(_) => continue,
                Err(std::sync::mpsc::RecvTimeoutError::Timeout)
                    if forward_done.load(Ordering::Acquire) =>
                {
                    return Ok(())
                }
                Err(std::sync::mpsc::RecvTimeoutError::Timeout) => continue,
                Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => return Ok(()),
            }
        }
    });
    let result = session.prompt(PromptRequest {
        request_id,
        prompt,
        goal,
    });
    prompt_done.store(true, Ordering::Release);
    forwarder
        .join()
        .map_err(|_| "event forwarder panicked".to_string())??;
    match result {
        Ok(value) => state.writer.send(&success_response(
            id,
            serde_json::to_value(value).map_err(|error| error.to_string())?,
        )),
        Err(error) => state
            .writer
            .send(&error_response(id, "prompt_failed", &error)),
    }
}

fn create_session(
    state: &Arc<ServerState>,
    params: Value,
    resume: bool,
) -> Result<Value, (&'static str, String)> {
    let options_value = params
        .get("options")
        .cloned()
        .unwrap_or_else(|| params.clone());
    let options: SessionOptions = serde_json::from_value(options_value)
        .map_err(|error| ("invalid_params", error.to_string()))?;
    let session = if resume {
        let source = string_param(&params, "session").map_err(|error| ("invalid_params", error))?;
        AgentSession::resume(options, source).map_err(|error| ("session_error", error))?
    } else {
        AgentSession::new(options).map_err(|error| ("session_error", error))?
    };
    session
        .set_interaction_handler(Some(state.bridge.clone()))
        .map_err(|error| ("session_error", error))?;
    let session_id = format!(
        "session-{}",
        state.next_session.fetch_add(1, Ordering::Relaxed)
    );
    let session_path = session
        .session_path()
        .map_err(|error| ("session_error", error))?;
    state
        .sessions
        .lock()
        .map_err(|_| ("internal_error", "sessions lock poisoned".into()))?
        .insert(session_id.clone(), session);
    Ok(json!({"sessionId": session_id, "sessionPath": session_path}))
}

fn abort_session(
    state: &Arc<ServerState>,
    params: &Value,
) -> Result<Value, (&'static str, String)> {
    let session = find_session(state, params)?;
    let request_id =
        string_param(params, "requestId").map_err(|error| ("invalid_params", error))?;
    Ok(json!({"aborted": session.abort(&request_id).map_err(|error| ("session_error", error))?}))
}

fn session_status(
    state: &Arc<ServerState>,
    params: &Value,
) -> Result<Value, (&'static str, String)> {
    let session = find_session(state, params)?;
    Ok(json!({"activeRequestIds": session.status().map_err(|error| ("session_error", error))?}))
}

fn dispose_session(
    state: &Arc<ServerState>,
    params: &Value,
) -> Result<Value, (&'static str, String)> {
    let session_id =
        string_param(params, "sessionId").map_err(|error| ("invalid_params", error))?;
    let session = state
        .sessions
        .lock()
        .map_err(|_| ("internal_error", "sessions lock poisoned".into()))?
        .remove(&session_id)
        .ok_or_else(|| {
            (
                "unknown_session",
                format!("unknown session: {}", session_id),
            )
        })?;
    session
        .dispose()
        .map_err(|error| ("session_error", error))?;
    Ok(json!({"disposed": true}))
}

fn resolve_elicitation(
    state: &Arc<ServerState>,
    params: &Value,
) -> Result<Value, (&'static str, String)> {
    let token = string_param(params, "token").map_err(|error| ("invalid_params", error))?;
    let answer = string_param(params, "answer");
    state
        .bridge
        .resolve_elicitation(&token, answer)
        .map_err(|error| ("interaction_error", error))?;
    Ok(json!({"accepted": true}))
}

fn resolve_approval(
    state: &Arc<ServerState>,
    params: &Value,
) -> Result<Value, (&'static str, String)> {
    let token = string_param(params, "token").map_err(|error| ("invalid_params", error))?;
    let approved = params
        .get("approved")
        .and_then(Value::as_bool)
        .ok_or_else(|| "approved must be a boolean".to_string());
    state
        .bridge
        .resolve_approval(&token, approved)
        .map_err(|error| ("interaction_error", error))?;
    Ok(json!({"accepted": true}))
}

fn find_session(
    state: &Arc<ServerState>,
    params: &Value,
) -> Result<AgentSession, (&'static str, String)> {
    let session_id =
        string_param(params, "sessionId").map_err(|error| ("invalid_params", error))?;
    state
        .sessions
        .lock()
        .map_err(|_| ("internal_error", "sessions lock poisoned".into()))?
        .get(&session_id)
        .cloned()
        .ok_or_else(|| {
            (
                "unknown_session",
                format!("unknown session: {}", session_id),
            )
        })
}

fn string_param(params: &Value, key: &str) -> Result<String, String> {
    params
        .get(key)
        .and_then(Value::as_str)
        .filter(|value| !value.trim().is_empty())
        .map(str::to_string)
        .ok_or_else(|| format!("{} must be a non-empty string", key))
}

fn wire_id(id: &Value) -> String {
    id.as_str()
        .map(str::to_string)
        .unwrap_or_else(|| id.to_string())
}

fn success_response(id: Value, result: Value) -> Value {
    json!({"id": id, "result": result})
}

fn error_response(id: Value, code: &str, message: &str) -> Value {
    json!({"id": id, "error": {"code": code, "message": message}})
}

fn read_frame<R: BufRead>(input: &mut R) -> Result<Option<Vec<u8>>, String> {
    let mut frame = Vec::new();
    loop {
        let available = input.fill_buf().map_err(|error| error.to_string())?;
        if available.is_empty() {
            return if frame.is_empty() {
                Ok(None)
            } else {
                Ok(Some(frame))
            };
        }
        let newline = available.iter().position(|byte| *byte == b'\n');
        let take = newline.map(|index| index + 1).unwrap_or(available.len());
        if frame.len().saturating_add(take) > MAX_FRAME_BYTES {
            input.consume(take);
            if newline.is_none() {
                discard_to_newline(input)?;
            }
            return Err(format!("frame exceeds {} bytes", MAX_FRAME_BYTES));
        }
        frame.extend_from_slice(&available[..take]);
        input.consume(take);
        if newline.is_some() {
            while matches!(frame.last(), Some(b'\n' | b'\r')) {
                frame.pop();
            }
            return Ok(Some(frame));
        }
    }
}

fn discard_to_newline<R: BufRead>(input: &mut R) -> Result<(), String> {
    loop {
        let available = input.fill_buf().map_err(|error| error.to_string())?;
        if available.is_empty() {
            return Ok(());
        }
        if let Some(index) = available.iter().position(|byte| *byte == b'\n') {
            input.consume(index + 1);
            return Ok(());
        }
        let len = available.len();
        input.consume(len);
    }
}
