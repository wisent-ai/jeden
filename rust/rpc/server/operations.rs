use super::*;
pub(super) fn workspace_status() -> Result<Value, (&'static str, String)> {
    let cwd = std::env::current_dir().map_err(|error| ("storage", error.to_string()))?;
    crate::cli::workspace::status(&cwd)
        .map(|report| {
            report
                .map(|report| report.value())
                .unwrap_or_else(|| json!({"status": "not_adopted"}))
        })
        .map_err(|error| ("invalid_workspace", error))
}

pub(super) fn workspace_discover(params: &Value) -> Result<Value, (&'static str, String)> {
    let cwd = std::env::current_dir().map_err(|error| ("storage", error.to_string()))?;
    let path = params
        .get("path")
        .and_then(Value::as_str)
        .map(PathBuf::from)
        .unwrap_or_else(|| cwd.clone());
    crate::cli::workspace::inspect(&path, &cwd, "discovered")
        .map(|report| report.value())
        .map_err(|error| ("invalid_workspace", error))
}

pub(super) fn workspace_adopt(params: &Value) -> Result<Value, (&'static str, String)> {
    let cwd = std::env::current_dir().map_err(|error| ("storage", error.to_string()))?;
    let path = string_param(params, "path")
        .map(PathBuf::from)
        .map_err(|error| ("invalid_request", error))?;
    crate::cli::workspace::adopt(&path, &cwd)
        .map(|report| report.value())
        .map_err(|error| ("invalid_workspace", error))
}
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

pub(super) fn string_param(params: &Value, key: &str) -> Result<String, String> {
    params
        .get(key)
        .and_then(Value::as_str)
        .filter(|value| !value.trim().is_empty())
        .map(str::to_string)
        .ok_or_else(|| format!("{} must be a non-empty string", key))
}

fn text_param(params: &Value, key: &str) -> Result<String, String> {
    params
        .get(key)
        .and_then(Value::as_str)
        .map(str::to_string)
        .ok_or_else(|| format!("{} must be a string", key))
}

pub(super) fn wire_id(id: &Value) -> String {
    id.as_str()
        .map(str::to_string)
        .unwrap_or_else(|| id.to_string())
}

pub(super) fn success_response(id: Value, result: Value) -> Value {
    json!({"id": id, "result": result})
}

pub(super) fn error_response(id: Value, code: &str, message: &str) -> Value {
    json!({"id": id, "error": {"code": code, "message": message}})
}

pub(super) fn read_frame<R: BufRead>(input: &mut R) -> Result<Option<Vec<u8>>, String> {
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
