mod context;
mod dispatch;
mod hosting;
mod lease;
mod operations;
mod parking;
mod prompt;
mod token;
use dispatch::handle_request;
use hosting::quick_replies;
pub use hosting::serve_headless_cli;
use operations::*;
use prompt::handle_prompt;
pub(crate) use token::request as credential_request;

use super::{
    AgentSessionFacade, BoundedExecutor, HeadlessConfig, HeadlessDaemon, IdempotencyStore,
    MtlsConfig, ReloadableTlsAcceptor, ReplayStore, SessionService, TenantDirectory, TenantError,
    TenantGuard, TenantLimits, WisentIdentityAuthority,
};
use rand::RngCore;
use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};

use super::interaction::RpcInteractionBridge;
use crate::rpc::server::operations::wire::error_response;
use crate::rpc::server::operations::wire::success_response;
use crate::rpc::server::operations::workspace::import_sessions;
use crate::rpc::server::operations::workspace::workspace_adopt;
use crate::rpc::server::operations::workspace::workspace_discover;
use crate::rpc::server::operations::workspace::workspace_status;
use crate::sdk::{AgentSession, SessionOptions};
use serde::Deserialize;
use serde_json::{json, Value};
use std::collections::HashMap;
use std::io::{self, BufRead, Write};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;

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
    context_requests: context::Requests,
    pub(super) bridge: Arc<RpcInteractionBridge>,
    pub(super) writer: JsonWriter,
    next_session: AtomicU64,
    shutting_down: AtomicBool,
}

/// One entry of the identity map. It names either a client certificate SAN
/// (`san` plus `principal`) or a Wisent organization whose members sign in
/// with their Wisent identity (`wisentOrganization`), never both.
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct HeadlessIdentityMapping {
    #[serde(default)]
    san: Option<String>,
    #[serde(default)]
    principal: Option<String>,
    #[serde(default)]
    wisent_organization: Option<uuid::Uuid>,
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
        context_requests: context::Requests::default(),
        bridge,
        writer: writer.clone(),
        next_session: AtomicU64::new(std::num::NonZeroU64::MIN.get()),
        shutting_down: AtomicBool::new(false),
    });
    writer.send(&json!({
        "type": "ready",
        "protocol": "jeden-rpc",
        "version": crate::rpc::PROTOCOL_VERSION.rsplit_once('v').map(|(_, version)| version)
            .ok_or("RPC protocol declaration has no version")?.parse::<u64>()
            .map_err(|error| format!("RPC protocol version is not a number: {error}"))?,
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
                        notice["reason"]
                            .as_str()
                            .ok_or("park notice missing reason")?,
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
            "session/prompt" | "session/completion/continue"
        ) {
            let worker_state = state.clone();
            let in_flight = activity.begin();
            workers.push(thread::spawn(move || {
                let _in_flight = in_flight;
                handle_prompt(worker_state, request)
            }));
        } else if matches!(
            request.method.as_str(),
            "context/recommend" | "context/sources"
        ) {
            let Some(context_request) = context::begin(&state, &request)? else {
                continue;
            };
            let worker_state = state.clone();
            let in_flight = activity.begin();
            workers.push(thread::spawn(move || {
                let _in_flight = in_flight;
                let result = match request.method.as_str() {
                    "context/recommend" => context::recommend(&worker_state, &request),
                    "context/sources" => context::sources(&worker_state, &request),
                    other => Err((
                        "method_not_found",
                        format!("unknown context method: {other}"),
                    )),
                };
                let response = match result {
                    Ok(value) => success_response(request.id, value),
                    Err((code, detail)) => error_response(request.id, code, &detail),
                };
                // Reuse of the id must not race this worker's teardown.
                drop(context_request);
                worker_state.writer.send(&response)
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
            session.abort(&request_id).map_err(|error| {
                format!("cannot cancel active RPC request {request_id} during shutdown: {error}")
            })?;
        }
    }
    state.bridge.cancel_all()?;
    state.context_requests.cancel_all();
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
