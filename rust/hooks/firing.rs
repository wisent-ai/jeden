//! Firing one lifecycle event's hooks with the payload that event carries:
//! a tool about to run or just run, a prompt, a session start, an answer.

use serde_json::{json, Value};
use std::path::Path;

use super::event;
use super::prompt_context;
use super::run::fire_event;

/// The name the shared section form is judged under by policy hooks that
/// read a call's kind: an anchored edit of the files it names.
const SECTION_TOOL: &str = "edit";

/// Fire `PreToolUse` for `tool`; returns `Some(reason)` if a hook blocks it.
///
/// A file-changing tool whose request does not itself carry the complete file
/// it leaves (Jeden's anchored edit, a line or replacement edit, a move or a
/// removal) is presented once per file in the shared section form, planned
/// from the same reading the tool executes: `tool_name` and `tool_input` carry
/// that section, while `tool` and `input` keep the request as the model sent
/// it. A request that cannot be planned is refused with its cause instead of
/// being handed to hooks that could not read it.
pub fn pretool_block(
    cwd: &Path,
    tool: &str,
    input: &Value,
    allow_project: bool,
    transcript_path: &Path,
) -> Option<String> {
    let subjects = match crate::tool_runtime::policy_subjects(cwd, tool, input) {
        Ok(Some(subjects)) => subjects,
        Ok(None) => {
            return pretool_subject(cwd, tool, tool, input, input, allow_project, transcript_path)
        }
        Err(cause) => {
            return Some(format!(
                "PreToolUse hooks cannot be shown the files {tool} would leave: {cause}"
            ))
        }
    };
    subjects.iter().find_map(|subject| {
        pretool_subject(cwd, tool, SECTION_TOOL, input, subject, allow_project, transcript_path)
    })
}

fn pretool_subject(
    cwd: &Path,
    tool: &str,
    judged_as: &str,
    input: &Value,
    subject: &Value,
    allow_project: bool,
    transcript_path: &Path,
) -> Option<String> {
    let payload = json!({
        "event": event::PRE_TOOL_USE,
        "hook_event_name": event::PRE_TOOL_USE,
        "tool": tool,
        "tool_name": judged_as,
        "input": input,
        "tool_input": subject,
        "cwd": cwd,
        "transcript_path": transcript_path,
    });
    let outcomes = fire_event(cwd, event::PRE_TOOL_USE, tool, &payload, allow_project);
    super::block_decision(&outcomes, "PreToolUse hook denied this tool")
}

/// Fire `PostToolUse` for `tool`. Nothing here changes the tool's result, but
/// a hook that failed, or a hook configuration that could not be read, is
/// said with its exit status and its own words rather than dropped.
pub fn posttool(cwd: &Path, tool: &str, result: &Value, allow_project: bool) {
    let payload =
        json!({ "event": event::POST_TOOL_USE, "tool": tool, "result": result, "cwd": cwd });
    for outcome in fire_event(cwd, event::POST_TOOL_USE, tool, &payload, allow_project) {
        if outcome.exit_code != super::tama::PASS_EXIT {
            eprintln!(
                "jeden: a PostToolUse hook for {tool} failed (exit {}): {}",
                outcome.exit_code,
                outcome.stderr.trim()
            );
        }
    }
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
