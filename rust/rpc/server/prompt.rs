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

pub(super) fn handle_prompt(state: Arc<ServerState>, request: WireRequest) -> Result<(), String> {
    let id = request.id.clone();
    if let Err(error) = handle_prompt_inner(&state, request) {
        state
            .writer
            .send(&error_response(id, "prompt_failed", &error))
            .map_err(|delivery| {
                format!("{error}; returning the RPC failure response also failed: {delivery}")
            })?;
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
    let subscription_id = subscription.id();
    let event_writer = state.writer.clone();
    let event_request_id = request_id.clone();
    // The forwarder blocks on the subscription. It stops at this request's
    // terminal event, or when the prompt has returned and the subscription is
    // ended below, after draining what was already queued.
    let forwarder = thread::spawn(move || -> Result<(), String> {
        while let Ok(event) = subscription.recv() {
            if event.request_id != event_request_id {
                continue;
            }
            let terminal = matches!(
                &event.event,
                SessionEventKind::Result { .. } | SessionEventKind::Error { .. }
            );
            event_writer.send(&json!({"method": "session/event", "params": event}))?;
            if terminal {
                return Ok(());
            }
        }
        Ok(())
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
    let completion_failure = |operation: &str, cause: String| {
        let failure = format!("{operation} for RPC request {id}: {cause}");
        match &result {
            Ok(_) => failure,
            Err(original) => format!("{original}; {failure}"),
        }
    };
    session
        .end_subscription(subscription_id)
        .map_err(|error| completion_failure("ending event subscription failed", error))?;
    forwarder
        .join()
        .map_err(|panic| {
            let cause = if let Some(message) = panic.downcast_ref::<String>() {
                message.clone()
            } else if let Some(message) = panic.downcast_ref::<&str>() {
                (*message).to_string()
            } else {
                format!(
                    "event forwarder panicked with non-string payload {:?}",
                    panic.as_ref().type_id()
                )
            };
            completion_failure("joining event forwarder failed", cause)
        })?
        .map_err(|error| completion_failure("forwarding session events failed", error))?;
    match result {
        Ok(value) => state.writer.send(&success_response(
            id,
            serde_json::to_value(value).map_err(|error| error.to_string())?,
        )),
        Err(error) => state
            .writer
            .send(&error_response(id, "prompt_failed", &error))
            .map_err(|delivery| {
                format!("{error}; sending the prompt failure response failed: {delivery}")
            }),
    }
}
