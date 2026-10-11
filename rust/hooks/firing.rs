//! Firing one lifecycle event's hooks with the payload that event carries:
//! a tool about to run or just run, a prompt, a session start, an answer.

use serde_json::{json, Value};
use std::path::Path;

use super::event;
use super::prompt_context;
use super::run::fire_event;

/// Fire `PreToolUse` for `tool`; returns `Some(reason)` if a hook blocks it.
pub fn pretool_block(
    cwd: &Path,
    tool: &str,
    input: &Value,
    allow_project: bool,
    transcript_path: &Path,
) -> Option<String> {
    let payload = json!({
        "event": event::PRE_TOOL_USE,
        "hook_event_name": event::PRE_TOOL_USE,
        "tool": tool,
        "tool_name": tool,
        "input": input,
        "tool_input": input,
        "cwd": cwd,
        "transcript_path": transcript_path,
    });
    let outcomes = fire_event(cwd, event::PRE_TOOL_USE, tool, &payload, allow_project);
    super::block_decision(&outcomes, "PreToolUse hook denied this tool")
}

/// Fire `PostToolUse` for `tool` (best-effort; outcomes are ignored).
pub fn posttool(cwd: &Path, tool: &str, result: &Value, allow_project: bool) {
    let payload =
        json!({ "event": event::POST_TOOL_USE, "tool": tool, "result": result, "cwd": cwd });
    let _ = fire_event(cwd, event::POST_TOOL_USE, tool, &payload, allow_project);
}

/// Fire `UserPromptSubmit`; returns injected context (joined hook stdout).
///
/// `automation` says Jeden itself wrote the prompt — a Pursuit stage or an
/// automatic continuation — so a hook that learns from the operator's words
/// (Tama's frustration drafter, the adaptive bridge) does not take a stage
/// instruction such as "do not execute commands" for his correction.
pub fn user_prompt_submit(
    cwd: &Path,
    prompt: &str,
    automation: bool,
    allow_project: bool,
) -> Result<String, String> {
    let author = if automation { "automation" } else { "operator" };
    let payload = json!({
        "event": event::USER_PROMPT_SUBMIT,
        "prompt": prompt,
        "prompt_author": author,
        "cwd": cwd,
    });
    let outcomes = fire_event(cwd, event::USER_PROMPT_SUBMIT, "", &payload, allow_project);
    prompt_context(&outcomes)
}

/// Fire SessionStart and return context for the model, not a terminal banner.
pub fn session_start(
    cwd: &Path,
    allow_project: bool,
    source: &str,
    session_id: &str,
    transcript_path: &Path,
) -> Result<String, String> {
    let payload = json!({
        "event": event::SESSION_START,
        "hook_event_name": event::SESSION_START,
        "cwd": cwd,
        "source": source,
        "session_id": session_id,
        "transcript_path": transcript_path,
    });
    let outcomes = fire_event(cwd, event::SESSION_START, "", &payload, allow_project);
    prompt_context(&outcomes)
}

/// Fire `Stop` for the answer the agent is about to give; `Some(reason)` means
/// a hook refused it, and the turn must continue with that reason instead of
/// ending. The payload is the shape every Stop hook reads (Claude's, which the
/// Tama catalog is written against): the session, its transcript, the pending
/// answer as `last_assistant_message`, and `stop_hook_active` once an earlier
/// answer of the same turn was already refused.
pub fn answer_stop_block(
    cwd: &Path,
    session_id: &str,
    transcript_path: &Path,
    answer: &str,
    stop_hook_active: bool,
    allow_project: bool,
) -> Option<String> {
    let payload = json!({
        "event": event::STOP,
        "hook_event_name": event::STOP,
        "cwd": cwd,
        "session_id": session_id,
        "transcript_path": transcript_path,
        "last_assistant_message": answer,
        "stop_hook_active": stop_hook_active,
    });
    let outcomes = fire_event(cwd, event::STOP, "", &payload, allow_project);
    super::block_decision(&outcomes, "Stop hook refused this answer")
}
