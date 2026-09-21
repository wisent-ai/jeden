//! Tama hook-registry loader: native support for the shared cross-agent hook
//! registry produced by hooks-rotator (`shared-hooks/registry.json`,
//! `managedBy: "jeden-unified-hooks"`). The registry speaks claude-style event
//! and tool names; this module maps them onto jeden's hook events and tool
//! matchers so the same catalog runs natively.
//!
//! Registry shape: a `version` number, a `managedBy` name, and an `events`
//! object keyed by claude-style event names such as `pre_tool_use:bash`. Each
//! event carries a `blocking` flag and a `hooks` array, and each entry there
//! carries `id`, `type` and `command`. A per-entry interval, if the registry
//! declares one, is the dispatcher's own data: jeden lets a guard finish.
//!
//! Source precedence: env `JEDEN_TAMA_REGISTRY` (path; empty string disables)
//! > config key `hooks.tamaRegistry` (path; empty disables) > auto-discovery of
//! > `~/Documents/CodingProjects/Wisent/hooks-rotator/shared-hooks/registry.json`
//! > and `~/.shared-hooks/registry.json`. No registry found = no hooks, quietly.

mod command;
mod events;

use serde_json::Value;
use std::path::{Path, PathBuf};

use super::Hook;

pub use events::{describe_source, load_event_hooks, normalize_outcome};

/// jeden's own block signal: a `PreToolUse` hook that exits with this code
/// refuses the tool, and `normalize_outcome` maps a Tama failure onto it.
pub(crate) const BLOCK_EXIT: i32 = 2;
/// An outcome that carries no verdict of its own.
const PASS_EXIT: i32 = 0;

/// One registry hook resolved for a jeden event: the jeden-shaped `Hook`
/// (matcher + command) plus the entry's blocking flag.
///
/// `unrunnable` carries the reason when the registration names an executable
/// this machine does not have. The hook still counts — a registered guard that
/// cannot run is never read as permission — but the turn is told what is
/// broken instead of being handed `sh: …: No such file or directory`.
#[derive(Debug, Clone, PartialEq)]
pub struct TamaHook {
    pub id: String,
    pub hook: Hook,
    pub blocking: bool,
    pub unrunnable: Option<String>,
}

/// Resolve the registry path by precedence; `None` disables the feature.
/// An explicitly configured path that does not exist also yields `None`
/// (quiet — no behavior change when no registry is found).
pub fn registry_path(cwd: &Path) -> Option<PathBuf> {
    if let Some(raw) = std::env::var_os("JEDEN_TAMA_REGISTRY") {
        return explicit_path(&raw.to_string_lossy());
    }
    let config = crate::cli::config::merged_config_value(cwd);
    if let Some(raw) =
        crate::cli::config::config_value_at(&config, "hooks.tamaRegistry").and_then(Value::as_str)
    {
        return explicit_path(raw);
    }
    let home = std::env::var_os("HOME").map(PathBuf::from)?;
    [
        home.join("Documents/CodingProjects/Wisent/hooks-rotator/shared-hooks/registry.json"),
        home.join(".shared-hooks/registry.json"),
    ]
    .into_iter()
    .find(|candidate| candidate.is_file())
}

fn explicit_path(raw: &str) -> Option<PathBuf> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return None; // empty string disables the registry entirely
    }
    let path = PathBuf::from(trimmed);
    path.is_file().then_some(path)
}

/// Map a Tama registry event name to jeden's `(event, tool matcher)` pair.
/// Unknown events yield `None` and are ignored.
pub(crate) fn map_event(tama_event: &str) -> Option<(&'static str, String)> {
    match tama_event {
        "user_prompt_submit" => Some((crate::hooks::event::USER_PROMPT_SUBMIT, String::new())),
        "stop" => Some((crate::hooks::event::STOP, String::new())),
        "session_start" | "session_start:compact" => {
            Some((crate::hooks::event::SESSION_START, String::new()))
        }
        _ => {
            let (kind, tool) = tama_event.split_once(':')?;
            let event = match kind {
                "pre_tool_use" => crate::hooks::event::PRE_TOOL_USE,
                "post_tool_use" => crate::hooks::event::POST_TOOL_USE,
                _ => return None,
            };
            Some((event, tool_matcher(tool)))
        }
    }
}

/// Claude tool name → anchored jeden tool-name matcher.
fn tool_matcher(tool: &str) -> String {
    let matcher = match tool {
        "bash" => "^(run_command|run_process)$",
        "read" => "^(read|read_file|read_binary_file|read_archive|read_document)$",
        "edit" => "^(edit|edit_file|apply_patch)$",
        "write" => "^(write|write_file)$",
        "multiedit" => "^(edit|apply_patch)$",
        "notebook" => "^read_document$",
        "task" => "^delegate_task$",
        "todo" => "^todo$",
        "eval" => "^(eval_session|python_eval|node_eval)$",
        "ssh" => "^ssh_exec$",
        "ask" => "^ask_user$",
        "lookup" => "^(search_files|search_text|grep_regex|glob_paths|ast_search)$",
        // jeden has no wait/goal tools; park both on the todo tool so these
        // guardrail hooks still fire against the closest equivalent surface.
        "wait" | "goal" => "^todo$",
        other => return format!("^{}$", regex::escape(other)),
    };
    matcher.to_string()
}

#[cfg(test)]
mod tests;
