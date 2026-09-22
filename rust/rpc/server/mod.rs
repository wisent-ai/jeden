mod context;
mod hosting;
mod operations;
mod parking;
mod prompt;
use hosting::quick_replies;
pub use hosting::serve_headless_cli;
use operations::*;
use prompt::handle_prompt;

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
pub(super) struct WireRequest {
    #[serde(default)]
    pub(super) id: Value,
    pub(super) method: String,
    #[serde(default)]
    pub(super) params: Value,
}

pub(super) struct ServerState {
    pub(super) sessions: Mutex<HashMap<String, AgentSession>>,
    pub(super) bridge: Arc<RpcInteractionBridge>,
    pub(super) writer: JsonWriter,
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

pub fn serve<R, W>(input: R, output: W) -> Result<(), String>
where
    R: BufRead + Send + 'static,
    W: Write + Send + 'static,
{
    let park = parking::ParkPolicy::from_environment()?;
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

    let inbox = parking::inbox(input, park.as_ref())?;
    let activity = parking::Activity::new();
    let mut workers = Vec::new();
    while !state.shutting_down.load(Ordering::Acquire) {
        let frame = match inbox.recv() {
            Ok(parking::Inbound::Frame(Ok(frame))) => frame,
            Ok(parking::Inbound::Frame(Err(error))) => {
                writer.send(&error_response(Value::Null, "malformed_frame", &error))?;
                continue;
            }
            Ok(parking::Inbound::Check) => {
                let notice = park
                    .as_ref()
                    .and_then(|policy| parking::park_notice(&state, &activity, policy));
                if let Some(notice) = notice {
                    eprintln!(
                        "jeden rpc parked ({}): nothing ran for {} s",
                        notice["reason"].as_str().unwrap_or_default(),
                        notice["quietSeconds"]
                    );
                    writer.send(&notice)?;
                    break;
                }
                continue;
            }
            Ok(parking::Inbound::Closed) | Err(_) => break,
        };
        activity.touch();
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
        if matches!(
            request.method.as_str(),
            "prompt" | "session/prompt" | "session/completion/continue"
        ) {
            let worker_state = state.clone();
            let in_flight = activity.begin();
            workers.push(thread::spawn(move || {
                let _in_flight = in_flight;
                handle_prompt(worker_state, request)
            }));
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
        "context/recommend" => context::recommend(&request.params),
        "context/sources" => context::sources(&request.params),
        "workspace/status" => workspace_status(),
        "workspace/discover" => workspace_discover(&request.params),
        "workspace/adopt" => workspace_adopt(&request.params),
        "session/import" => import_sessions(&request.params),
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

