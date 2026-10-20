//! The RPC method table, shared by stdio and headless callers.
use super::*;

mod chat_memory;

pub(super) fn handle_request(state: &Arc<ServerState>, request: WireRequest) -> Result<(), Value> {
    let id = request.id.clone();
    let result = match request.method.as_str() {
        "initialize" => Ok(json!({
            "protocol": "jeden-rpc",
            "capabilities": AgentSession::capabilities(),
            "quickReplies": quick_replies()
        })),
        "session/new" => create_session(state, request.params, false),
        "config/contracts/get" => Ok(crate::cli::config::schema::contract_settings()),
        "config/contracts/set" => set_contract_settings(&request.params),
        "config/communication/get" => Ok(crate::cli::config::schema::communication_settings()),
        "config/communication/set" => set_communication_settings(&request.params),
        "config/task-limits/get" => Ok(crate::task_runtime::limits_settings()),
        "config/task-limits/set" => request
            .params
            .get("limits")
            .cloned()
            .ok_or_else(|| {
                (
                    "invalid_params",
                    "config/task-limits/set requires limits".to_string(),
                )
            })
            .and_then(|limits| {
                crate::task_runtime::set_limits(limits)
                    .map_err(|error| ("invalid_params", error.to_string()))
            }),
        "context/cancel" => context::cancel(state, &request.params),
        "workspace/status" => workspace_status(),
        "workspace/discover" => workspace_discover(&request.params),
        "workspace/adopt" => workspace_adopt(&request.params),
        "session/import" => import_sessions(&request.params),
        "session/open" | "session/load" => create_session(state, request.params, true),
        "session/cancel" => abort_session(state, &request.params),
        "session/status" => session_status(state, &request.params),
        "session/completion/get" | "session/completion/control" | "session/completion/add" => {
            completion_request(state, &request.params, &request.method)
        }
        "asks/list" | "asks/get" | "asks/answer" => asks_request(&request.params, &request.method),
        "session/dispose" => dispose_session(state, &request.params, false),
        "session/retire" => dispose_session(state, &request.params, true),
        "session/input_response" => resolve_elicitation(state, &request.params),
        "session/permission_response" => resolve_approval(state, &request.params),
        "shutdown" => {
            state.shutting_down.store(true, Ordering::Release);
            Ok(json!({"shuttingDown": true}))
        }
        method if method.starts_with("token/") => token::request(method, &request.params),
        method if method.starts_with("chat-memory/") => {
            chat_memory::request(method, &request.params)
        }
        _ => match request.method.as_str() {
            "lease/list" => lease::list(&request.params),
            "lease/release" => lease::release(&request.params),
            "lease/end" => lease::end(&request.params),
            other => Err(("method_not_found", format!("unknown method: {other}"))),
        },
    };
    match result {
        Ok(value) => state
            .writer
            .send(&success_response(id, value))
            .map_err(|error| error_response(Value::Null, "write_error", &error)),
        Err((code, message)) => Err(error_response(id, code, &message)),
    }
}
