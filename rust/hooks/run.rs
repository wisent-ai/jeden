use serde_json::{json, Value};
use std::io::Write;
use std::path::Path;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use super::{
    hook_matches, parse_event_hooks, project_hooks_path, prompt_context, read_config,
    resolve_trusted_hooks, user_hooks_path, Hook, HookOutcome, HOOK_TIMEOUT,
};

/// Run one hook command via `sh -c`, feeding `payload` JSON on stdin, capped by
/// `HOOK_TIMEOUT`. A spawn/timeout failure surfaces as a non-zero outcome
/// rather than aborting the turn.
pub fn run_hook(cwd: &Path, hook: &Hook, payload: &Value) -> HookOutcome {
    let child = Command::new("sh")
        .arg("-c")
        .arg(&hook.command)
        .current_dir(cwd)
        .env(
            "JEDEN_HOOK_EVENT",
            payload.get("event").and_then(Value::as_str).unwrap_or(""),
        )
        .env(
            "JEDEN_HOOK_TOOL",
            payload.get("tool").and_then(Value::as_str).unwrap_or(""),
        )
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn();
    let mut child = match child {
        Ok(c) => c,
        Err(e) => {
            return HookOutcome {
                exit_code: -1,
                stdout: String::new(),
                stderr: format!("hook spawn failed: {e}"),
            };
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
                    return HookOutcome {
                        exit_code: -1,
                        stdout: String::new(),
                        stderr: "hook timed out".into(),
                    };
                }
                std::thread::sleep(Duration::from_millis(10));
            }
            Err(e) => {
                return HookOutcome {
                    exit_code: -1,
                    stdout: String::new(),
                    stderr: format!("hook wait failed: {e}"),
                };
            }
        }
    }
    let output = match child.wait_with_output() {
        Ok(o) => o,
        Err(e) => {
            return HookOutcome {
                exit_code: -1,
                stdout: String::new(),
                stderr: format!("hook output failed: {e}"),
            }
        }
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
pub fn fire_event(
    cwd: &Path,
    event: &str,
    tool: &str,
    payload: &Value,
    allow_project: bool,
) -> Vec<HookOutcome> {
    let project = read_config(&project_hooks_path(cwd));
    let user = user_hooks_path()
        .map(|p| read_config(&p))
        .unwrap_or(Value::Null);
    let mut hooks = resolve_trusted_hooks(&user, &project, event, tool, allow_project);
    // Installed plugins contribute their own hooks.json. User-scope plugin hooks
    // always run; project-scope plugin hooks obey the same `allow_project` gate.
    for config in crate::slash::installed_plugin_hook_configs(cwd, allow_project) {
        let mut plugin_hooks = parse_event_hooks(&config, event);
        if !tool.is_empty() {
            plugin_hooks.retain(|h| hook_matches(h, tool));
        }
        hooks.extend(plugin_hooks);
    }
    let mut outcomes = hooks
        .iter()
        .map(|hook| run_hook(cwd, hook, payload))
        .collect::<Vec<_>>();
    match super::extensions::fire_hooks(cwd, event, tool, payload, allow_project) {
        Ok(values) => outcomes.extend(values.into_iter().map(|value| HookOutcome {
            exit_code: 0,
            stdout: match value {
                Value::String(text) => text,
                Value::Null => String::new(),
                other => other.to_string(),
            },
            stderr: String::new(),
        })),
        Err(error) => outcomes.push(HookOutcome {
            exit_code: -1,
            stdout: String::new(),
            stderr: format!("extension hook dispatch failed: {error}"),
        }),
    }
    outcomes
}

/// Fire `PreToolUse` for `tool`; returns `Some(reason)` if a hook blocks it.
pub fn pretool_block(cwd: &Path, tool: &str, input: &Value, allow_project: bool) -> Option<String> {
    let payload = json!({ "event": "PreToolUse", "tool": tool, "input": input, "cwd": cwd });
    let outcomes = fire_event(cwd, "PreToolUse", tool, &payload, allow_project);
    super::pretool_block_decision(&outcomes)
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
