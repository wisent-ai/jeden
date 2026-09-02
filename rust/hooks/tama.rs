//! Tama hook-registry loader: native support for the shared cross-agent hook
//! registry produced by hooks-rotator (`shared-hooks/registry.json`,
//! `managedBy: "jeden-unified-hooks"`). The registry speaks claude-style event
//! and tool names; this module maps them onto jeden's hook events and tool
//! matchers so the same catalog runs natively.
//!
//! Registry shape:
//! ```json
//! { "version": 1, "managedBy": "jeden-unified-hooks", "events": {
//!     "pre_tool_use:bash": { "blocking": true, "hooks": [
//!         { "id": "…", "type": "command", "command": "…", "timeout": 10 } ] }
//! } }
//! ```
//!
//! Source precedence: env `JEDEN_TAMA_REGISTRY` (path; empty string disables)
//! > config key `hooks.tamaRegistry` (path; empty disables) > auto-discovery of
//! > `~/Documents/CodingProjects/Wisent/hooks-rotator/shared-hooks/registry.json`
//! > and `~/.shared-hooks/registry.json`. No registry found = silently no hooks.

use serde_json::Value;
use std::path::{Path, PathBuf};
use std::time::Duration;

use super::{hook_matches, parse_hook_json, read_config, Hook, HookOutcome, HOOK_TIMEOUT};

/// One registry hook resolved for a jeden event: the jeden-shaped `Hook`
/// (matcher + command) plus the entry's own timeout and blocking flag.
#[derive(Debug, Clone, PartialEq)]
pub struct TamaHook {
    pub id: String,
    pub hook: Hook,
    pub timeout: Duration,
    pub blocking: bool,
}

/// Resolve the registry path by precedence; `None` disables the feature.
/// An explicitly configured path that does not exist also yields `None`
/// (silent — no behavior change when no registry is found).
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
        "user_prompt_submit" => Some(("UserPromptSubmit", String::new())),
        "stop" => Some(("Stop", String::new())),
        "session_start" | "session_start:compact" => Some(("SessionStart", String::new())),
        _ => {
            let (kind, tool) = tama_event.split_once(':')?;
            let event = match kind {
                "pre_tool_use" => "PreToolUse",
                "post_tool_use" => "PostToolUse",
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

/// The command of an executable registry entry: a non-empty `command` with no
/// `type` or `type: "command"`. Other entry kinds cannot run natively here.
fn entry_command(entry: &Value) -> Option<String> {
    match entry.get("type").and_then(Value::as_str) {
        Some("command") | None => {}
        Some(_) => return None,
    }
    let command = entry.get("command").and_then(Value::as_str)?.trim();
    if command.is_empty() {
        None
    } else {
        Some(command.to_string())
    }
}

/// Registry hooks for jeden `event` (`PreToolUse`, `UserPromptSubmit`, …),
/// filtered to `tool` (empty = no tool filter). Empty vec when no registry.
pub fn load_event_hooks(cwd: &Path, event: &str, tool: &str) -> Vec<TamaHook> {
    let Some(path) = registry_path(cwd) else {
        return Vec::new();
    };
    let registry = read_config(&path);
    let Some(events) = registry.get("events").and_then(Value::as_object) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for (tama_event, spec) in events {
        let Some((mapped, matcher)) = map_event(tama_event) else {
            continue;
        };
        if mapped != event {
            continue;
        }
        let event_blocking = spec
            .get("blocking")
            .and_then(Value::as_bool)
            .unwrap_or(false);
        let Some(entries) = spec.get("hooks").and_then(Value::as_array) else {
            continue;
        };
        for entry in entries {
            let Some(command) = entry_command(entry) else {
                continue;
            };
            let hook = Hook {
                matcher: matcher.clone(),
                command,
            };
            if !tool.is_empty() && !hook_matches(&hook, tool) {
                continue;
            }
            let timeout = entry
                .get("timeout")
                .and_then(Value::as_u64)
                .filter(|secs| *secs > 0)
                .map(Duration::from_secs)
                .unwrap_or(HOOK_TIMEOUT);
            let blocking = entry
                .get("blocking")
                .and_then(Value::as_bool)
                .unwrap_or(event_blocking);
            let id = entry
                .get("id")
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_string();
            out.push(TamaHook {
                id,
                hook,
                timeout,
                blocking,
            });
        }
    }
    out
}

/// Adapt a Tama hook outcome to jeden's decision contract. jeden blocks on
/// exit code 2 or stdout JSON `{"decision":"block"}` (see
/// `pretool_block_decision`); a Tama `blocking` event treats any failure
/// (non-zero exit, spawn/timeout error) as a block, with an explicit JSON
/// `{"decision":"approve"}` overriding. Non-blocking events only record
/// outcomes, so their block signals are scrubbed before they can reach jeden's
/// decision path (only `PreToolUse` outcomes feed it).
pub fn normalize_outcome(event: &str, blocking: bool, outcome: HookOutcome) -> HookOutcome {
    let decision = parse_hook_json(&outcome.stdout)
        .as_ref()
        .and_then(|json| json.get("decision"))
        .and_then(Value::as_str)
        .map(str::to_ascii_lowercase);
    let approve = decision.as_deref() == Some("approve");
    let block_verdict = decision.as_deref() == Some("block");
    let failed = outcome.exit_code != 0;
    if blocking && !approve && (failed || block_verdict) {
        // Exit code 2 is jeden's block signal; stdout/stderr carry the reason.
        HookOutcome {
            exit_code: 2,
            ..outcome
        }
    } else if !blocking && event == "PreToolUse" && (outcome.exit_code == 2 || block_verdict) {
        HookOutcome {
            exit_code: 0,
            stdout: if block_verdict {
                String::new()
            } else {
                outcome.stdout
            },
            stderr: outcome.stderr,
        }
    } else {
        outcome
    }
}

/// `/hooks` summary of the resolved registry: one row per mapped Tama event
/// that has at least one executable hook. `None` when no registry is
/// configured or it holds nothing runnable (silent).
pub fn describe_source(cwd: &Path) -> Option<String> {
    let path = registry_path(cwd)?;
    let registry = read_config(&path);
    let events = registry.get("events").and_then(Value::as_object)?;
    let mut rows = Vec::new();
    for (tama_event, spec) in events {
        let Some((event, matcher)) = map_event(tama_event) else {
            continue;
        };
        let count = spec
            .get("hooks")
            .and_then(Value::as_array)
            .map(|entries| {
                entries
                    .iter()
                    .filter(|e| entry_command(e).is_some())
                    .count()
            })
            .unwrap_or(0);
        if count == 0 {
            continue;
        }
        let blocking = spec
            .get("blocking")
            .and_then(Value::as_bool)
            .unwrap_or(false);
        let matcher = if matcher.is_empty() {
            "*".to_string()
        } else {
            matcher
        };
        rows.push(format!(
            "  {tama_event} -> {event} [{matcher}] {count} hook(s){}",
            if blocking { ", blocking" } else { "" }
        ));
    }
    if rows.is_empty() {
        return None;
    }
    let managed_by = registry
        .get("managedBy")
        .and_then(Value::as_str)
        .unwrap_or("unknown");
    Some(format!(
        "Tama registry ({}, managedBy {}):\n{}",
        path.display(),
        managed_by,
        rows.join("\n")
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mapped(tama_event: &str) -> (&'static str, String) {
        map_event(tama_event).expect("mapped event")
    }

    #[test]
    fn event_mapping() {
        assert_eq!(
            mapped("user_prompt_submit"),
            ("UserPromptSubmit", String::new())
        );
        assert_eq!(mapped("stop"), ("Stop", String::new()));
        assert_eq!(mapped("session_start"), ("SessionStart", String::new()));
        assert_eq!(
            mapped("session_start:compact"),
            ("SessionStart", String::new())
        );
        assert_eq!(
            mapped("pre_tool_use:bash"),
            ("PreToolUse", "^(run_command|run_process)$".to_string())
        );
        assert_eq!(
            mapped("post_tool_use:bash"),
            ("PostToolUse", "^(run_command|run_process)$".to_string())
        );
        assert!(map_event("unknown_event").is_none());
    }

    #[test]
    fn tool_matchers() {
        assert_eq!(
            tool_matcher("read"),
            "^(read|read_file|read_binary_file|read_archive|read_document)$"
        );
        assert_eq!(tool_matcher("edit"), "^(edit|edit_file|apply_patch)$");
        assert_eq!(tool_matcher("write"), "^(write|write_file)$");
        assert_eq!(tool_matcher("multiedit"), "^(edit|apply_patch)$");
        assert_eq!(tool_matcher("notebook"), "^read_document$");
        assert_eq!(tool_matcher("task"), "^delegate_task$");
        assert_eq!(tool_matcher("todo"), "^todo$");
        assert_eq!(
            tool_matcher("eval"),
            "^(eval_session|python_eval|node_eval)$"
        );
        assert_eq!(tool_matcher("ssh"), "^ssh_exec$");
        assert_eq!(tool_matcher("ask"), "^ask_user$");
        assert_eq!(
            tool_matcher("lookup"),
            "^(search_files|search_text|grep_regex|glob_paths|ast_search)$"
        );
        assert_eq!(tool_matcher("wait"), "^todo$");
        assert_eq!(tool_matcher("goal"), "^todo$");
        assert_eq!(tool_matcher("functions_ask"), "^functions_ask$");
    }

    #[test]
    fn outcome_normalization() {
        let failed = HookOutcome {
            exit_code: 3,
            stdout: String::new(),
            stderr: "denied".into(),
        };
        // Blocking events: any failure becomes jeden's exit-2 block signal.
        assert_eq!(
            normalize_outcome("PreToolUse", true, failed.clone()).exit_code,
            2
        );
        // ... unless the hook explicitly approves.
        let approve = HookOutcome {
            exit_code: 3,
            stdout: "{\"decision\":\"approve\"}".into(),
            stderr: String::new(),
        };
        assert_eq!(normalize_outcome("PreToolUse", true, approve).exit_code, 3);
        // A block verdict blocks even on a zero exit.
        let verdict = HookOutcome {
            exit_code: 0,
            stdout: "{\"decision\":\"block\",\"reason\":\"no\"}".into(),
            stderr: String::new(),
        };
        assert_eq!(normalize_outcome("PreToolUse", true, verdict).exit_code, 2);
        // Non-blocking events never block: exit 2 is scrubbed.
        assert_eq!(normalize_outcome("PreToolUse", false, failed).exit_code, 3);
        let exit_two = HookOutcome {
            exit_code: 2,
            stdout: String::new(),
            stderr: "denied".into(),
        };
        assert_eq!(
            normalize_outcome("PreToolUse", false, exit_two).exit_code,
            0
        );
        let verdict_nonblocking = normalize_outcome(
            "PreToolUse",
            false,
            HookOutcome {
                exit_code: 0,
                stdout: "{\"decision\":\"block\"}".into(),
                stderr: String::new(),
            },
        );
        assert_eq!(verdict_nonblocking.exit_code, 0);
        assert!(verdict_nonblocking.stdout.is_empty());
    }
}
