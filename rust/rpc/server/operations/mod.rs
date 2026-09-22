//! What each protocol request does: sessions, completions, and the answers to
//! the questions a running turn asks.

use super::*;

pub(crate) mod wire;
pub(crate) mod workspace;

pub(crate) use wire::{error_response, string_param, success_response, wire_id};
use wire::text_param;

pub(super) fn create_session(
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
        AgentSession::resume_in_place(options, source).map_err(|error| ("session_error", error))?
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

pub(super) fn abort_session(
    state: &Arc<ServerState>,
    params: &Value,
) -> Result<Value, (&'static str, String)> {
    let session = find_session(state, params)?;
    let request_id =
        string_param(params, "requestId").map_err(|error| ("invalid_params", error))?;
    Ok(json!({"aborted": session.abort(&request_id).map_err(|error| ("session_error", error))?}))
}

pub(super) fn session_status(
    state: &Arc<ServerState>,
    params: &Value,
) -> Result<Value, (&'static str, String)> {
    let session = find_session(state, params)?;
    Ok(json!({"activeRequestIds": session.status().map_err(|error| ("session_error", error))?}))
}

pub(super) fn completion_request(
    state: &Arc<ServerState>,
    params: &Value,
    action: &str,
) -> Result<Value, (&'static str, String)> {
    let session = find_session(state, params)?;
    let completion = if action.ends_with("/add") {
        let prompt = string_param(params, "prompt").map_err(|error| ("invalid_params", error))?;
        session.add_request(&prompt)
    } else if action.ends_with("/control") {
        let task_id = string_param(params, "taskId").map_err(|error| ("invalid_params", error))?;
        let action = string_param(params, "action").map_err(|error| ("invalid_params", error))?;
        let reason = string_param(params, "reason").map_err(|error| ("invalid_params", error))?;
        let revision = params.get("revision").and_then(Value::as_u64).ok_or((
            "invalid_params",
            "revision must be an unsigned integer".into(),
        ))?;
        session.control_completion(&task_id, &action, &reason, revision)
    } else {
        session.completion()
    }
    .map_err(|error| ("completion_error", error))?;
    Ok(json!({"sessionId": params["sessionId"], "completion": completion}))
}

pub(super) fn dispose_session(
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

pub(super) fn resolve_elicitation(
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

pub(super) fn resolve_approval(
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

pub(super) fn set_contract_settings(params: &Value) -> Result<Value, (&'static str, String)> {
    let communication =
        text_param(params, "communication").map_err(|error| ("invalid_params", error))?;
    let functionality =
        text_param(params, "functionality").map_err(|error| ("invalid_params", error))?;
    crate::cli::config::schema::set_contract_settings(&communication, &functionality)
        .map_err(|error| ("config_write_failed", error))
}

pub(super) fn set_communication_settings(params: &Value) -> Result<Value, (&'static str, String)> {
    use crate::cli::config::schema::{
        COMMUNICATION_CODE_KEY, COMMUNICATION_MODE_KEY, COMMUNICATION_REASONING_KEY,
        COMMUNICATION_TOOL_CALLS_KEY, COMMUNICATION_TOOL_RESULTS_KEY,
    };
    let mode = string_param(params, "mode").map_err(|error| ("invalid_params", error))?;
    let tool_calls =
        string_param(params, "toolCalls").map_err(|error| ("invalid_params", error))?;
    let tool_results =
        string_param(params, "toolResults").map_err(|error| ("invalid_params", error))?;
    let reasoning = string_param(params, "reasoning").map_err(|error| ("invalid_params", error))?;
    let code = string_param(params, "code").map_err(|error| ("invalid_params", error))?;
    crate::cli::config::schema::set_communication_settings(&[
        (COMMUNICATION_MODE_KEY, mode.as_str()),
        (COMMUNICATION_TOOL_CALLS_KEY, tool_calls.as_str()),
        (COMMUNICATION_TOOL_RESULTS_KEY, tool_results.as_str()),
        (COMMUNICATION_REASONING_KEY, reasoning.as_str()),
        (COMMUNICATION_CODE_KEY, code.as_str()),
    ])
    .map_err(|error| ("config_write_failed", error))
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

