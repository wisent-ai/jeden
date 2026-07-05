//! Lifecycle hook runtime: user- and project-defined shell commands that fire
//! on agent events (OMP extension/hook parity). A `PreToolUse` hook can block a
//! tool by exiting with code 2; `UserPromptSubmit` hooks inject their stdout as
//! extra context; `PostToolUse`/`SessionStart`/`Stop` run best-effort.
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
use serde_json::{json, Value};
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

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

const HOOK_TIMEOUT: Duration = Duration::from_secs(30);

fn project_hooks_path(cwd: &Path) -> PathBuf {
    cwd.join(".jeden/hooks.json")
}

fn user_hooks_path() -> Option<PathBuf> {
    std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".jeden/hooks.json"))
}

/// Read a hooks config file into a JSON `Value`; a missing or malformed file
/// yields `Value::Null` (treated as "no hooks") rather than an error.
fn read_config(path: &Path) -> Value {
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
                    let command = entry.get("command").and_then(Value::as_str)?.trim().to_string();
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
pub fn resolve_trusted_hooks(user: &Value, project: &Value, event: &str, tool: &str, allow_project: bool) -> Vec<Hook> {
    let mut hooks = parse_event_hooks(user, event);
    if allow_project {
        hooks.extend(parse_event_hooks(project, event));
    }
    if tool.is_empty() {
        hooks
    } else {
        hooks.into_iter().filter(|h| hook_matches(h, tool)).collect()
    }
}

/// A `PreToolUse` block decision: `Some(reason)` blocks the tool. A hook blocks
/// by exiting with code 2; the reason is its stderr (or stdout) if present.
pub fn pretool_block_decision(outcomes: &[HookOutcome]) -> Option<String> {
    outcomes.iter().find(|o| o.exit_code == 2).map(|o| {
        let reason = if !o.stderr.trim().is_empty() {
            o.stderr.trim()
        } else {
            o.stdout.trim()
        };
        if reason.is_empty() {
            "PreToolUse hook denied this tool".to_string()
        } else {
            reason.to_string()
        }
    })
}

/// Non-empty stdout lines across `UserPromptSubmit` hook outcomes, joined — this
/// is injected into the prompt as extra context.
pub fn prompt_context(outcomes: &[HookOutcome]) -> String {
    outcomes
        .iter()
        .map(|o| o.stdout.trim())
        .filter(|s| !s.is_empty())
        .collect::<Vec<_>>()
        .join("\n")
}

/// Run one hook command via `sh -c`, feeding `payload` JSON on stdin, capped by
/// `HOOK_TIMEOUT`. A spawn/timeout failure surfaces as a non-zero outcome
/// rather than aborting the turn.
pub fn run_hook(cwd: &Path, hook: &Hook, payload: &Value) -> HookOutcome {
    let child = Command::new("sh")
        .arg("-c")
        .arg(&hook.command)
        .current_dir(cwd)
        .env("JEDEN_HOOK_EVENT", payload.get("event").and_then(Value::as_str).unwrap_or(""))
        .env("JEDEN_HOOK_TOOL", payload.get("tool").and_then(Value::as_str).unwrap_or(""))
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn();
    let mut child = match child {
        Ok(c) => c,
        Err(e) => {
            return HookOutcome { exit_code: -1, stdout: String::new(), stderr: format!("hook spawn failed: {e}") };
        }
    };
    if let Some(mut stdin) = child.stdin.take() {
        let _ = stdin.write_all(payload.to_string().as_bytes());
    }
    let started = Instant::now();
    loop {
        match child.try_wait() {
            Ok(Some(_)) => break,
            Ok(None) => {
                if started.elapsed() > HOOK_TIMEOUT {
                    let _ = child.kill();
                    let _ = child.wait();
                    return HookOutcome { exit_code: -1, stdout: String::new(), stderr: "hook timed out".into() };
                }
                std::thread::sleep(Duration::from_millis(10));
            }
            Err(e) => {
                return HookOutcome { exit_code: -1, stdout: String::new(), stderr: format!("hook wait failed: {e}") };
            }
        }
    }
    let output = match child.wait_with_output() {
        Ok(o) => o,
        Err(e) => return HookOutcome { exit_code: -1, stdout: String::new(), stderr: format!("hook output failed: {e}") },
    };
    HookOutcome {
        exit_code: output.status.code().unwrap_or(-1),
        stdout: String::from_utf8_lossy(&output.stdout).to_string(),
        stderr: String::from_utf8_lossy(&output.stderr).to_string(),
    }
}

/// Load merged config from disk (user + project) for one event, filter by tool,
/// run each matching hook, and collect outcomes. Project hooks run only when
/// `allow_project` (see [`resolve_trusted_hooks`]). Missing configs = no hooks.
pub fn fire_event(cwd: &Path, event: &str, tool: &str, payload: &Value, allow_project: bool) -> Vec<HookOutcome> {
    let project = read_config(&project_hooks_path(cwd));
    let user = user_hooks_path().map(|p| read_config(&p)).unwrap_or(Value::Null);
    let hooks = resolve_trusted_hooks(&user, &project, event, tool, allow_project);
    hooks.iter().map(|hook| run_hook(cwd, hook, payload)).collect()
}

/// Fire `PreToolUse` for `tool`; returns `Some(reason)` if a hook blocks it.
pub fn pretool_block(cwd: &Path, tool: &str, input: &Value, allow_project: bool) -> Option<String> {
    let payload = json!({ "event": "PreToolUse", "tool": tool, "input": input, "cwd": cwd });
    let outcomes = fire_event(cwd, "PreToolUse", tool, &payload, allow_project);
    pretool_block_decision(&outcomes)
}

/// Fire `PostToolUse` for `tool` (best-effort; outcomes are ignored).
pub fn posttool(cwd: &Path, tool: &str, result: &Value, allow_project: bool) {
    let payload = json!({ "event": "PostToolUse", "tool": tool, "result": result, "cwd": cwd });
    let _ = fire_event(cwd, "PostToolUse", tool, &payload, allow_project);
}

/// Fire `UserPromptSubmit`; returns injected context (joined hook stdout).
pub fn user_prompt_submit(cwd: &Path, prompt: &str, allow_project: bool) -> String {
    let payload = json!({ "event": "UserPromptSubmit", "prompt": prompt, "cwd": cwd });
    let outcomes = fire_event(cwd, "UserPromptSubmit", "", &payload, allow_project);
    prompt_context(&outcomes)
}

/// Fire `SessionStart` at the beginning of a session; returns joined hook
/// stdout (a banner/context line the caller may surface).
pub fn session_start(cwd: &Path, allow_project: bool) -> String {
    let payload = json!({ "event": "SessionStart", "cwd": cwd });
    let outcomes = fire_event(cwd, "SessionStart", "", &payload, allow_project);
    prompt_context(&outcomes)
}

/// Fire `Stop` at the end of a session (best-effort side effects).
pub fn session_stop(cwd: &Path, allow_project: bool) {
    let payload = json!({ "event": "Stop", "cwd": cwd });
    let _ = fire_event(cwd, "Stop", "", &payload, allow_project);
}

/// Human summary of configured hooks (for `/hooks`), split by trust origin.
/// Only lists the events the runtime actually fires.
pub fn describe_hooks(cwd: &Path) -> String {
    let project = read_config(&project_hooks_path(cwd));
    let user = user_hooks_path().map(|p| read_config(&p)).unwrap_or(Value::Null);
    let events = ["SessionStart", "UserPromptSubmit", "PreToolUse", "PostToolUse", "Stop"];
    let mut lines = Vec::new();
    for (label, config) in [("User (~/.jeden/hooks.json, always trusted)", &user), ("Project (.jeden/hooks.json, runs only with --allow-command)", &project)] {
        let mut section = Vec::new();
        for event in events {
            let hooks = parse_event_hooks(config, event);
            for hook in hooks {
                let m = if hook.matcher.is_empty() { "*".to_string() } else { hook.matcher.clone() };
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

#[cfg(test)]
mod tests {
    use super::*;

    fn cfg(event: &str, entries: Value) -> Value {
        json!({ "version": 1, "hooks": { event: entries } })
    }

    #[test]
    fn parse_event_hooks_extracts_command_and_matcher() {
        let c = cfg("PreToolUse", json!([{ "matcher": "run_command", "command": "echo hi" }]));
        let hooks = parse_event_hooks(&c, "PreToolUse");
        assert_eq!(hooks, vec![Hook { matcher: "run_command".into(), command: "echo hi".into() }]);
    }

    #[test]
    fn parse_event_hooks_skips_empty_commands_and_missing_event() {
        let c = cfg("PreToolUse", json!([{ "command": "  " }, { "matcher": "x" }]));
        assert!(parse_event_hooks(&c, "PreToolUse").is_empty());
        assert!(parse_event_hooks(&c, "PostToolUse").is_empty());
    }

    #[test]
    fn parse_event_hooks_defaults_matcher_to_empty() {
        let c = cfg("PostToolUse", json!([{ "command": "log.sh" }]));
        let hooks = parse_event_hooks(&c, "PostToolUse");
        assert_eq!(hooks[0].matcher, "");
    }

    #[test]
    fn hook_matches_empty_matcher_matches_all() {
        let h = Hook { matcher: "".into(), command: "x".into() };
        assert!(hook_matches(&h, "run_command"));
        assert!(hook_matches(&h, "read_file"));
    }

    #[test]
    fn hook_matches_regex_alternation() {
        let h = Hook { matcher: "run_command|write_file".into(), command: "x".into() };
        assert!(hook_matches(&h, "run_command"));
        assert!(hook_matches(&h, "write_file"));
        assert!(!hook_matches(&h, "read_file"));
    }

    #[test]
    fn hook_matches_invalid_regex_fails_closed() {
        let h = Hook { matcher: "[unclosed".into(), command: "x".into() };
        assert!(!hook_matches(&h, "run_command"));
    }

    #[test]
    fn resolve_trusted_hooks_gates_project_but_not_user() {
        let user = cfg("PreToolUse", json!([{ "command": "user.sh" }]));
        let project = cfg("PreToolUse", json!([{ "command": "project.sh" }]));
        // Untrusted: only the user hook runs.
        let denied = resolve_trusted_hooks(&user, &project, "PreToolUse", "", false);
        assert_eq!(denied.len(), 1);
        assert_eq!(denied[0].command, "user.sh");
        // Trusted: both run, user first.
        let allowed = resolve_trusted_hooks(&user, &project, "PreToolUse", "", true);
        assert_eq!(allowed.len(), 2);
        assert_eq!(allowed[0].command, "user.sh");
        assert_eq!(allowed[1].command, "project.sh");
    }

    #[test]
    fn resolve_trusted_hooks_runs_no_project_hooks_when_denied_even_with_empty_user() {
        let project = cfg("PreToolUse", json!([{ "command": "danger.sh" }]));
        let hooks = resolve_trusted_hooks(&Value::Null, &project, "PreToolUse", "", false);
        assert!(hooks.is_empty(), "cloned-repo project hooks must not run without --allow-command");
    }

    #[test]
    fn resolve_hooks_orders_user_before_project_and_filters_by_tool() {
        let user = cfg("PreToolUse", json!([{ "matcher": "run_command", "command": "user.sh" }]));
        let project = cfg("PreToolUse", json!([
            { "matcher": "run_command", "command": "project.sh" },
            { "matcher": "read_file", "command": "skip.sh" },
        ]));
        let hooks = resolve_trusted_hooks(&user, &project, "PreToolUse", "run_command", true);
        assert_eq!(hooks.len(), 2);
        assert_eq!(hooks[0].command, "user.sh");
        assert_eq!(hooks[1].command, "project.sh");
    }

    #[test]
    fn resolve_hooks_no_tool_filter_when_tool_empty() {
        let project = cfg("UserPromptSubmit", json!([{ "matcher": "ignored", "command": "ctx.sh" }]));
        let hooks = resolve_trusted_hooks(&Value::Null, &project, "UserPromptSubmit", "", true);
        assert_eq!(hooks.len(), 1);
    }

    #[test]
    fn pretool_block_decision_blocks_on_exit_2_with_stderr_reason() {
        let outcomes = vec![
            HookOutcome { exit_code: 0, stdout: "ok".into(), stderr: "".into() },
            HookOutcome { exit_code: 2, stdout: "".into(), stderr: "not allowed".into() },
        ];
        assert_eq!(pretool_block_decision(&outcomes), Some("not allowed".to_string()));
    }

    #[test]
    fn pretool_block_decision_falls_back_to_stdout_then_default() {
        let a = vec![HookOutcome { exit_code: 2, stdout: "stdout reason".into(), stderr: "".into() }];
        assert_eq!(pretool_block_decision(&a), Some("stdout reason".to_string()));
        let b = vec![HookOutcome { exit_code: 2, stdout: "".into(), stderr: "".into() }];
        assert_eq!(pretool_block_decision(&b), Some("PreToolUse hook denied this tool".to_string()));
    }

    #[test]
    fn pretool_block_decision_none_when_no_exit_2() {
        let outcomes = vec![
            HookOutcome { exit_code: 0, stdout: "".into(), stderr: "".into() },
            HookOutcome { exit_code: 1, stdout: "".into(), stderr: "soft error".into() },
        ];
        assert_eq!(pretool_block_decision(&outcomes), None);
    }

    #[test]
    fn prompt_context_joins_nonempty_stdout() {
        let outcomes = vec![
            HookOutcome { exit_code: 0, stdout: "line one\n".into(), stderr: "".into() },
            HookOutcome { exit_code: 0, stdout: "  ".into(), stderr: "".into() },
            HookOutcome { exit_code: 0, stdout: "line two".into(), stderr: "".into() },
        ];
        assert_eq!(prompt_context(&outcomes), "line one\nline two");
    }
}
