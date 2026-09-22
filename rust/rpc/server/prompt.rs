//! Running one prompt for a connected client, and streaming the session's
//! events back as they happen.
//!
//! Split out of `rpc/server/mod.rs`, which had grown past the module line cap.

use super::operations::{error_response, string_param, success_response, wire_id};
use super::{ServerState, WireRequest};
use crate::sdk::{PromptRequest, SessionEventKind};
use serde_json::{json, Value};
use std::sync::Arc;
use std::thread;
use std::sync::atomic::AtomicBool;
use std::sync::atomic::Ordering;
use std::time::Duration;

pub(super) fn handle_prompt(state: Arc<ServerState>, request: WireRequest) -> Result<(), String> {
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
    let prompt = if continuing {
        String::new()
    } else {
        string_param(&request.params, "prompt")?
    };
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
        session.prompt(PromptRequest {
            request_id,
            prompt,
            goal,
        })
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
