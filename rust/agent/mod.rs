use rand::{distributions::Alphanumeric, Rng};
use serde_json::{json, Value};
use std::env;
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::collections::BTreeSet;
use std::time::{SystemTime, UNIX_EPOCH};

use crate::model_router::{chat_completion, chat_completion_streaming, ChatConfig, CompletionUsage};
use crate::protocol::{extract_json_object, parse_action, Action, ToolAction};
use crate::{handle_slash, load_config, session_root, Args, Config};

mod approval;
mod commands;
mod conversation;
mod hooks;
mod runtime;
mod state;


#[allow(unused_imports)]
pub(crate) use commands::{arm_force_tool, btw_command_with, btw_task, retry_command_with, retry_task, run_command, run_command_with};
pub(crate) use conversation::Conversation;
pub(crate) use hooks::{is_command_tool, is_write_tool, RunHooks, RunResult};
pub(crate) use runtime::model_router_config;
pub(crate) use state::{loop_next_prompt, record_branch, update_last_session_path, update_task_outcome, MAX_LOOP_ITERS};

use approval::{resolve_tool_approval, ToolDecision};
use runtime::{
    append_usage_event, env_usize, is_context_overflow_error, is_incomplete_output_error, now_stamp, rust_tool_specs,
    system_prompt, usage_cost, SessionRecorder,
};
use state::{apply_mode_instructions, capture_plan_if_enabled, read_mode_state, write_mode_state};
