use serde_json::{json, Value};
use std::io::Write;
use std::path::Path;
use std::process::{Command, Stdio};

use super::event;

use super::{
    hook_matches, parse_event_hooks, project_hooks_path, prompt_context, read_config,
    resolve_trusted_hooks, unreadable_config, user_hooks_path, Hook, HookOutcome,
};

/// The user and project configs, and why any config that applies could not
/// be read. The project config applies only when `allow_project`.
fn trusted_configs(cwd: &Path, allow_project: bool) -> (Value, Value, Vec<String>) {
    let mut failures = Vec::new();
    let mut loaded = |read: Result<Value, String>, applies: bool| match read {
        Ok(config) => config,
        Err(reason) => {
            if applies {
                failures.push(reason);
            }
            Value::Null
        }
    };
    let project = loaded(read_config(&project_hooks_path(cwd)), allow_project);
    let user = match user_hooks_path() {
        Some(path) => loaded(read_config(&path), true),
        None => Value::Null,
    };
    (user, project, failures)
}

/// Run one hook command via `sh -c`, feeding `payload` JSON on stdin. A hook
/// decides when it has finished deciding: a guard that reads a transcript or
/// asks a model takes as long as that takes, and killing it turns a verdict
/// nobody saw into an allowed action. A spawn failure surfaces as a non-zero
/// outcome rather than aborting the turn.
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
                stderr: format!(
                    "hook spawn failed for `{}` in {}: {e}",
                    hook.command,
                    cwd.display()
                ),
            };
        }
    };
    if let Some(mut stdin) = child.stdin.take() {
        let _ = stdin.write_all(payload.to_string().as_bytes());
    }
    let output = match child.wait_with_output() {
        Ok(o) => o,
        Err(e) => {
            return HookOutcome {
                exit_code: -1,
                stdout: String::new(),
                stderr: format!(
                    "hook output failed for `{}` in {}: {e}",
                    hook.command,
                    cwd.display()
                ),
            }
        }
    };
    let mut outcome = HookOutcome {
        exit_code: output.status.code().unwrap_or(-1),
        stdout: String::from_utf8_lossy(&output.stdout).to_string(),
        stderr: String::from_utf8_lossy(&output.stderr).to_string(),
    };
    if outcome.exit_code.is_negative() {
        outcome.stderr = format!(
            "hook process `{}` in {} ended without an exit code ({}): {}",
            hook.command,
            cwd.display(),
            output.status,
            outcome.stderr
        );
    }
    outcome
}

/// Load merged config from disk (user + project) for one event, filter by tool,
/// run each matching hook, and collect outcomes. Project hooks run only when
/// `allow_project` (see [`resolve_trusted_hooks`]). A missing config is no
/// hooks; a config that applies and cannot be read or is not JSON answers a
/// refusal naming it, ahead of every hook that did load.
pub fn fire_event(
    cwd: &Path,
    event: &str,
    tool: &str,
    payload: &Value,
    allow_project: bool,
) -> Vec<HookOutcome> {
    let (user, project, mut failures) = trusted_configs(cwd, allow_project);
    let mut hooks = resolve_trusted_hooks(&user, &project, event, tool, allow_project);
    // Installed plugins contribute their own hooks.json. User-scope plugin hooks
    // always run; project-scope plugin hooks obey the same `allow_project` gate.
    for config in crate::slash::installed_plugin_hook_configs(cwd, allow_project) {
        let config = match config {
            Ok(config) => config,
            Err(reason) => {
                failures.push(reason);
                continue;
            }
        };
        let mut plugin_hooks = parse_event_hooks(&config, event);
        if !tool.is_empty() {
            plugin_hooks.retain(|h| hook_matches(h, tool));
        }
        hooks.extend(plugin_hooks);
    }
    let mut outcomes = failures
        .into_iter()
        .map(unreadable_config)
        .collect::<Vec<_>>();
    outcomes.extend(hooks.iter().map(|hook| run_hook(cwd, hook, payload)));
    // Tama registry hooks (claude-style shared catalog) run after jeden's own
    // hooks. The registry lives with the operator's other user-level config,
    // so it is trusted like user hooks and not gated on `allow_project`.
    let source = payload.get("source").and_then(Value::as_str);
    for tama in super::tama::load_event_hooks(cwd, event, tool, source) {
        // A registration whose file this machine does not have never reaches
        // `sh`: spawning it answers `No such file or directory`, which reads
        // like a verdict on the tool rather than a broken hook install.
        let outcome = match tama.unrunnable {
            Some(reason) => HookOutcome {
                exit_code: super::tama::BLOCK_EXIT,
                stdout: String::new(),
                stderr: reason,
            },
            None => run_hook(cwd, &tama.hook, payload),
        };
        outcomes.push(super::tama::normalize_outcome(
            event,
            tama.blocking,
            outcome,
        ));
    }
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

/// Whether `fire_event` would run anything for `event`: a user, project,
/// plugin, Tama registry or extension hook. A turn holds back the text of its
/// answer while a `Stop` hook can still refuse it, so that a refused answer
/// is never streamed to the operator before the refusal.
pub fn has_event_hooks(cwd: &Path, event: &str, allow_project: bool) -> bool {
    let (user, project, failures) = trusted_configs(cwd, allow_project);
    !failures.is_empty()
        || !resolve_trusted_hooks(&user, &project, event, "", allow_project).is_empty()
        || crate::slash::installed_plugin_hook_configs(cwd, allow_project)
            .iter()
            .any(|config| match config {
                Ok(config) => !parse_event_hooks(config, event).is_empty(),
                Err(_) => true,
            })
        || !super::tama::load_event_hooks(cwd, event, "", None).is_empty()
        || super::extensions::has_hooks(cwd, event)
}

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
