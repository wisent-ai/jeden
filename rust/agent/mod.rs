use rand::{distributions::Alphanumeric, Rng};
use serde_json::{json, Value};
use std::collections::BTreeSet;
use std::env;
use std::fs::{self, OpenOptions};
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use crate::model_router::{
    chat_completion, chat_completion_streaming, ChatConfig, CompletionUsage,
};
use crate::protocol::{extract_json_object, parse_action, Action, ToolAction};
use crate::{handle_slash, load_config, session_root, Args, Config};

mod approval;
mod commands;
mod conversation;
pub(crate) mod credential;
mod hooks;
mod runtime;
mod state;

pub(crate) use commands::{arm_force_tool, btw_task, retry_task, run_command};
pub(crate) use conversation::Conversation;
pub(crate) use hooks::{is_command_tool, is_write_tool, RunHooks, RunResult, TraceEvent};
pub(crate) use runtime::communication_contract;

/// The router configuration, with Jeden's own credential accounted for.
///
/// `credential::ensure` has usually already run by here, on the first
/// control-plane read of this process. This call is what turns a refusal into
/// a sentence the operator sees, once, at the start of a turn: a run that
/// cannot sign a request used to fail with `BRAMA_URL is required` and no
/// account of what could not be read.
pub(crate) fn model_router_config(config: &Config, args: &Args) -> crate::model_router::ChatConfig {
    let (secret, bearer) = credential::ensure();
    for (variable, source) in [(credential::SECRET, secret), (credential::BEARER, bearer)] {
        if let Some(said) = source.refusal() {
            eprintln!("jeden: {variable} is unavailable: {said}");
        }
    }
    let mut router = runtime::model_router_config(config, args);
    if let Some(unread) = catalog_left_unread(&router) {
        eprintln!(
            "jeden: the Brama catalog could not be read ({unread}); continuing with the configured route `{}`",
            router.model
        );
        router.config_error = None;
    }
    router
}

/// The catalog read that left the catalog unread rather than answering about
/// the configured model.
///
/// Discovery is how a model name becomes a readable refusal; it is not the
/// authority on whether a route works, because the gateway that serves the
/// request is. On 2026-09-10 three retained assignments ended at
/// `Work remains open (task_intake): Brama transport error ...` because one
/// `GET /v1/models` timed out while chat calls in the same minute were being
/// served. A configured route now reaches the gateway and the request itself
/// answers. The prefixes below are `BramaError`'s own sentences, the only
/// shape of that failure which survives into `ChatConfig`; an explicit
/// refusal (`"retryable": false`), a missing model or any other configuration
/// error still stops the run here.
fn catalog_left_unread(router: &ChatConfig) -> Option<&str> {
    let error = router.config_error.as_deref()?;
    if router.model.trim().is_empty() || error.contains("\"retryable\":false") {
        return None;
    }
    let unread = error.starts_with("Brama transport error")
        || error.starts_with("Brama rate limited the request")
        || http_status(error).is_some_and(|status| (500..600).contains(&status));
    unread.then_some(error)
}

/// The HTTP status a `BramaError::Http` sentence carries, if it is one.
fn http_status(error: &str) -> Option<u16> {
    error
        .strip_prefix("Brama returned HTTP ")?
        .split(':')
        .next()?
        .trim()
        .parse()
        .ok()
}

pub(crate) use runtime::now_stamp;
pub(crate) use runtime::specs::system_prompt_checked;
pub(crate) use runtime::task_contract;
pub(crate) use state::{
    loop_next_prompt, record_branch, update_last_session_path, update_task_outcome, MAX_LOOP_ITERS,
};

use approval::{resolve_tool_approval, ToolDecision};
use runtime::{
    append_usage_event, env_usize, is_context_overflow_error, is_incomplete_output_error,
    prepare_outbound_messages, rust_tool_specs, usage_cost, SessionRecorder,
};
use state::{apply_mode_instructions, capture_plan_if_enabled, read_mode_state, write_mode_state};

pub(crate) fn record_roadmap_event(
    cwd: &Path,
    event_type: &str,
    data: Value,
) -> Result<PathBuf, String> {
    let active_roadmap_item = (event_type == "roadmap_item_started")
        .then(|| {
            data.get("itemId")
                .and_then(Value::as_str)
                .map(str::to_string)
        })
        .flatten();
    let last_session = read_mode_state(cwd)
        .pointer("/lastSessionPath")
        .and_then(Value::as_str)
        .map(PathBuf::from)
        .filter(|path| path.join("state.json").is_file());
    let mut recorder = match last_session {
        Some(path) => SessionRecorder::open(cwd, &path)?,
        None => SessionRecorder::new(cwd),
    };
    recorder.record(event_type, data)?;
    let path = recorder.path();
    if let Some(item_id) = active_roadmap_item {
        let metadata = serde_json::to_string_pretty(&json!({
            "schemaVersion": 1,
            "itemId": item_id,
            "activatedAt": now_stamp()
        }))
        .map_err(|error| error.to_string())?
            + "\n";
        fs::write(path.join("roadmap-item.json"), metadata).map_err(|error| error.to_string())?;
    }
    crate::slash::update_session_pointer(cwd, &path)?;
    Ok(path)
}

pub(crate) fn is_verification_read_tool(tool: &str) -> bool {
    approval::is_builtin_read_tool(tool) && tool != "ask_user"
}
