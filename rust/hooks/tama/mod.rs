//! Tama hook-registry loader: native support for the shared cross-agent hook
//! registry Tama installs (`~/.shared-hooks/registry.json`). The registry
//! speaks claude-style event and tool names; this module maps them onto
//! jeden's hook events and tool matchers so the same catalog runs natively.
//!
//! Registry shape: a `version` number, a `managedBy` name, and an `events`
//! object keyed by claude-style event names such as `pre_tool_use:bash`. Each
//! event carries a `blocking` flag and a `hooks` array, and each entry there
//! carries `id`, `type` and `command`. A per-entry interval, if the registry
//! declares one, is the dispatcher's own data: jeden lets a guard finish.
//!
//! Source precedence: env `JEDEN_TAMA_REGISTRY` (path; empty string disables)
//! > config key `hooks.tamaRegistry` (path; empty disables) > the registry
//! > Tama installed at `~/.shared-hooks/registry.json`. No installed registry
//! > = no hooks, quietly; a registry named explicitly that does not exist is
//! > refused naming it, never read as "no hooks". No source checkout is
//! > looked at.

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

/// Resolve the registry path by precedence; `Ok(None)` disables the feature
/// (an empty explicit path, or no Tama installed under the home). A path
/// named explicitly that does not exist is refused naming it and where it
/// was named: answered as `None`, a typo in it turned off every Tama guard
/// without a word.
pub fn registry_path(cwd: &Path) -> Result<Option<PathBuf>, String> {
    if let Some(raw) = std::env::var_os("JEDEN_TAMA_REGISTRY") {
        return explicit_path(&raw.to_string_lossy(), "JEDEN_TAMA_REGISTRY");
    }
    let config = crate::cli::config::merged_config_value(cwd);
    if let Some(raw) =
        crate::cli::config::config_value_at(&config, "hooks.tamaRegistry").and_then(Value::as_str)
    {
        return explicit_path(raw, "the configuration key hooks.tamaRegistry");
    }
    // The one home lookup: read from HOME alone, an unset HOME skipped the
    // installed registry and every Tama guard with it.
    let installed = crate::slash::common::dirs_home().join(".shared-hooks/registry.json");
    Ok(installed.is_file().then_some(installed))
}

fn explicit_path(raw: &str, named_by: &str) -> Result<Option<PathBuf>, String> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return Ok(None); // empty string disables the registry entirely
    }
    let path = PathBuf::from(trimmed);
    if path.is_file() {
        Ok(Some(path))
    } else {
        Err(format!(
            "the Tama registry {} named by {named_by} is not a file, so no Tama hook can run",
            path.display()
        ))
    }
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

/// The registry events Jeden has no event for, each with the ids of the
/// hooks that therefore do not run in Jeden, for `jeden doctor`. No
/// registry installed is none; one that cannot be read, or holds no events
/// table, is refused with its cause.
pub fn unmapped_events(cwd: &Path) -> Result<Vec<Value>, String> {
    let Some(path) = registry_path(cwd)? else {
        return Ok(Vec::new());
    };
    let registry = crate::hooks::read_config(&path)?;
    let events = registry
        .get("events")
        .and_then(Value::as_object)
        .ok_or_else(|| format!("the Tama registry {} holds no events table", path.display()))?;
    Ok(events
        .iter()
        .filter(|(name, _)| map_event(name).is_none())
        .map(|(name, spec)| {
            let hooks: Vec<&str> = spec
                .get("hooks")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
                .filter_map(|entry| entry.get("id").and_then(Value::as_str))
                .collect();
            serde_json::json!({ "event": name, "hooks": hooks })
        })
        .collect())
}

/// Claude tool name → anchored jeden tool-name matcher. `any` is the
/// registry's word for every tool (`post_tool_use:any`), not a tool's name:
/// matched literally it named no Jeden tool and its hooks never ran.
fn tool_matcher(tool: &str) -> String {
    let matcher = match tool {
        "any" => "^.+$",
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
        "wait" => "^todo$",
        "goal" => "^todo$",
        other => return format!("^{}$", regex::escape(other)),
    };
    matcher.to_string()
}
