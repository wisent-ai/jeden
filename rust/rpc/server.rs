mod hosting;
mod operations;
pub use hosting::serve_headless_cli;
use hosting::quick_replies;
use operations::*;

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
        "capabilities": AgentSession::capabilities(),
        "quickReplies": quick_replies()
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
        if matches!(request.method.as_str(), "prompt" | "session/prompt" | "session/completion/continue") {
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
            "capabilities": AgentSession::capabilities(),
            "quickReplies": quick_replies()
        })),
        "session/new" | "new" => create_session(state, request.params, false),
        "config/contracts/get" => Ok(crate::cli::config::schema::contract_settings()),
        "config/contracts/set" => set_contract_settings(&request.params),
        "config/communication/get" => Ok(crate::cli::config::schema::communication_settings()),
        "config/communication/set" => set_communication_settings(&request.params),
        "workspace/status" => workspace_status(),
        "workspace/discover" => workspace_discover(&request.params),
        "workspace/adopt" => workspace_adopt(&request.params),
        "session/open" | "session/load" | "resume" => create_session(state, request.params, true),
        "abort" | "session/cancel" => abort_session(state, &request.params),
        "status" | "session/status" => session_status(state, &request.params),
        "session/completion/get" | "session/completion/control" | "session/completion/add" => {
            completion_request(state, &request.params, &request.method)
        }
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
    let continuing = request.method == "session/completion/continue";
    let prompt = if continuing { String::new() } else { string_param(&request.params, "prompt")? };
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
    let result = if continuing {
        session.continue_work(request_id)
    } else {
        session.prompt(PromptRequest { request_id, prompt, goal })
    };
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

