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
mod hooks;
mod runtime;
mod state;

pub(crate) use commands::{arm_force_tool, btw_task, retry_task, run_command};
pub(crate) use conversation::Conversation;
pub(crate) use hooks::{is_command_tool, is_write_tool, RunHooks, RunResult};
pub(crate) use runtime::model_router_config;
pub(crate) use runtime::now_stamp;
pub(crate) use state::{
    loop_next_prompt, record_branch, update_last_session_path, update_task_outcome, MAX_LOOP_ITERS,
};

use approval::{resolve_tool_approval, ToolDecision};
use runtime::{
    append_usage_event, env_usize, is_context_overflow_error, is_incomplete_output_error,
    prepare_outbound_messages, rust_tool_specs, system_prompt_checked, usage_cost, SessionRecorder,
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
