//! Lifecycle hook runtime: user- and project-defined shell commands that fire
//! on agent events. A `PreToolUse` hook can block a tool by exiting with code 2;
//! `UserPromptSubmit` hooks inject their stdout as extra context;
//! `PostToolUse`/`SessionStart`/`Stop` run best-effort.
//!
//! Config lives in `.jeden/hooks.json` (project) and `~/.jeden/hooks.json`
//! (user). Both are merged — user hooks run first, then project hooks. Schema:
//! ```json
//! { "version": 1, "hooks": {
//!     "PreToolUse":  [ { "matcher": "run_command|write_file", "command": "..." } ],
//!     "PostToolUse": [ { "command": "..." } ],
//!     "UserPromptSubmit": [ { "command": "..." } ]
//! } }
//! ```

use regex::Regex;
use serde_json::Value;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::Duration;

#[path = "../extensions/mod.rs"]
mod extensions;
mod run;

pub(crate) use extensions::{
    agent_dirs as extension_agent_dirs, capability_descriptors as extension_capability_descriptors,
    command_dirs as extension_command_dirs, execute_tool as execute_extension_tool,
    model_entries as extension_model_entries, prompt_context as extension_prompt_context,
    skill_context as extension_skill_context,
};
pub use extensions::{
    provider_entries as extension_provider_entries, reload as reload_extensions,
    status as extension_status, ReloadReport,
};
pub use run::{posttool, pretool_block, session_start, session_stop, user_prompt_submit};

/// One configured hook: an optional matcher (regex over the tool name; empty =
/// match everything) and the shell command to run.
#[derive(Debug, Clone, PartialEq)]
pub struct Hook {
    pub matcher: String,
    pub command: String,
}

/// Outcome of running one hook command.
#[derive(Debug, Clone, PartialEq)]
pub struct HookOutcome {
    pub exit_code: i32,
    pub stdout: String,
    pub stderr: String,
}

pub(crate) const HOOK_TIMEOUT: Duration = Duration::from_secs(30);

pub(crate) fn project_hooks_path(cwd: &Path) -> PathBuf {
    cwd.join(".jeden/hooks.json")
}

pub(crate) fn user_hooks_path() -> Option<PathBuf> {
    std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".jeden/hooks.json"))
}

/// Read a hooks config file into a JSON `Value`; a missing or malformed file
/// yields `Value::Null` (treated as "no hooks") rather than an error.
pub(crate) fn read_config(path: &Path) -> Value {
    fs::read_to_string(path)
        .ok()
        .and_then(|text| serde_json::from_str(&text).ok())
        .unwrap_or(Value::Null)
}

/// Parse the hooks for one event out of a config `Value` (`{version, hooks:{}}`).
/// Unknown/missing shapes yield an empty list rather than erroring.
pub fn parse_event_hooks(config: &Value, event: &str) -> Vec<Hook> {
    config
        .get("hooks")
        .and_then(|h| h.get(event))
        .and_then(Value::as_array)
        .map(|arr| {
            arr.iter()
                .filter_map(|entry| {
                    let command = entry
                        .get("command")
                        .and_then(Value::as_str)?
                        .trim()
                        .to_string();
                    if command.is_empty() {
                        return None;
                    }
                    let matcher = entry
                        .get("matcher")
                        .and_then(Value::as_str)
                        .unwrap_or("")
                        .trim()
                        .to_string();
                    Some(Hook { matcher, command })
                })
                .collect()
        })
        .unwrap_or_default()
}

/// Whether `hook`'s matcher applies to `tool`. An empty matcher matches every
/// tool. An invalid regex never matches (fails closed for tool gating).
pub fn hook_matches(hook: &Hook, tool: &str) -> bool {
    if hook.matcher.is_empty() {
        return true;
    }
    match Regex::new(&hook.matcher) {
        Ok(re) => re.is_match(tool),
        Err(_) => false,
    }
}

/// Merge user + project configs for one event into an ordered hook list that
/// applies to `tool` (user hooks first, then project; `tool` empty = no matcher
/// filter), enforcing the trust boundary: user hooks (`~/.jeden/hooks.json`, the
/// operator's own machine) always run; project hooks (`.jeden/hooks.json`,
/// potentially from a cloned repo) run only when `allow_project` is set (i.e.
/// `--allow-command`). This keeps a cloned repo from silently executing shell.
pub fn resolve_trusted_hooks(
    user: &Value,
    project: &Value,
    event: &str,
    tool: &str,
    allow_project: bool,
) -> Vec<Hook> {
    let mut hooks = parse_event_hooks(user, event);
    if allow_project {
        hooks.extend(parse_event_hooks(project, event));
    }
    if tool.is_empty() {
        hooks
    } else {
        hooks
            .into_iter()
            .filter(|h| hook_matches(h, tool))
            .collect()
    }
}

/// Parse a hook's stdout as a JSON object, if it is one.
fn parse_hook_json(stdout: &str) -> Option<Value> {
    let trimmed = stdout.trim();
    if !trimmed.starts_with('{') {
        return None;
    }
    serde_json::from_str::<Value>(trimmed)
        .ok()
        .filter(|v| v.is_object())
}

/// A `PreToolUse` block decision: `Some(reason)` blocks the tool. A hook blocks
/// either by exiting with code 2, or by printing JSON `{"decision":"block",
/// "reason":"…"}` on stdout. The reason is the JSON `reason`, else stderr, else
/// stdout (or a default).
pub fn pretool_block_decision(outcomes: &[HookOutcome]) -> Option<String> {
    outcomes.iter().find_map(|o| {
        let json = parse_hook_json(&o.stdout);
        let json_block = json
            .as_ref()
            .and_then(|j| j.get("decision"))
            .and_then(Value::as_str)
            .map(|d| d.eq_ignore_ascii_case("block"))
            .unwrap_or(false);
        if o.exit_code != 2 && !json_block {
            return None;
        }
        let json_reason = json
            .as_ref()
            .and_then(|j| j.get("reason").or_else(|| j.get("userMessage")))
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|s| !s.is_empty());
        let reason = json_reason
            .map(str::to_string)
            .or_else(|| {
                Some(o.stderr.trim())
                    .filter(|s| !s.is_empty())
                    .map(str::to_string)
            })
            .or_else(|| {
                Some(o.stdout.trim())
                    .filter(|s| !s.is_empty() && parse_hook_json(&o.stdout).is_none())
                    .map(str::to_string)
            })
            .unwrap_or_else(|| "PreToolUse hook denied this tool".to_string());
        Some(reason)
    })
}

/// Injected context across prompt/session hook outcomes: each hook contributes
/// its JSON `additionalContext` field if present, else its raw stdout. Joined.
pub fn prompt_context(outcomes: &[HookOutcome]) -> String {
    outcomes
        .iter()
        .filter_map(|o| {
            if let Some(json) = parse_hook_json(&o.stdout) {
                let ctx = json
                    .get("additionalContext")
                    .and_then(Value::as_str)
                    .unwrap_or("")
                    .trim()
                    .to_string();
                if ctx.is_empty() {
                    None
                } else {
                    Some(ctx)
                }
            } else {
                let s = o.stdout.trim();
                if s.is_empty() {
                    None
                } else {
                    Some(s.to_string())
                }
            }
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// Human summary of configured hooks (for `/hooks`), split by trust origin.
/// Only lists the events the runtime actually fires.
pub fn describe_hooks(cwd: &Path) -> String {
    let project = read_config(&project_hooks_path(cwd));
    let user = user_hooks_path()
        .map(|p| read_config(&p))
        .unwrap_or(Value::Null);
    let events = [
        "SessionStart",
        "UserPromptSubmit",
        "PreToolUse",
        "PostToolUse",
        "Stop",
    ];
    let mut lines = Vec::new();
    for (label, config) in [
        ("User (~/.jeden/hooks.json, always trusted)", &user),
        (
            "Project (.jeden/hooks.json, runs only with --allow-command)",
            &project,
        ),
    ] {
        let mut section = Vec::new();
        for event in events {
            let hooks = parse_event_hooks(config, event);
            for hook in hooks {
                let m = if hook.matcher.is_empty() {
                    "*".to_string()
                } else {
                    hook.matcher.clone()
                };
                section.push(format!("  {} [{}] {}", event, m, hook.command));
            }
        }
        if !section.is_empty() {
            lines.push(label.to_string());
            lines.extend(section);
        }
    }
    if lines.is_empty() {
        format!(
            "No hooks configured.\nAdd them to {} (project) or ~/.jeden/hooks.json (user).\nEvents: SessionStart, UserPromptSubmit, PreToolUse (exit 2 blocks), PostToolUse, Stop.\nProject hooks run only with --allow-command.",
            project_hooks_path(cwd).display()
        )
    } else {
        lines.join("\n")
    }
}
