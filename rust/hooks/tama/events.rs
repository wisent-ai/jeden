//! Reading the registry for one jeden event, and adapting what its hooks
//! answer to jeden's own decision contract.

use serde_json::Value;
use std::path::Path;

use super::command::{entry_command, unrunnable_reason, EntryCommand};
use super::{map_event, registry_path, TamaHook, BLOCK_EXIT, PASS_EXIT};
use crate::hooks::{hook_matches, parse_hook_json, read_config, Hook, HookOutcome};

/// Registry hooks for jeden `event` (`PreToolUse`, `UserPromptSubmit`, …),
/// filtered to `tool` (empty = no tool filter). Empty vec when no registry.
pub fn load_event_hooks(cwd: &Path, event: &str, tool: &str) -> Vec<TamaHook> {
    let Some(path) = registry_path(cwd) else {
        return Vec::new();
    };
    let registry = read_config(&path);
    let catalog = registry.get("catalog").cloned().unwrap_or(Value::Null);
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
            let Some(resolved) = entry_command(entry, &path, &catalog) else {
                continue;
            };
            let id = entry
                .get("id")
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_string();
            let (command, unrunnable) = match resolved {
                EntryCommand::Runnable(command) => (command, None),
                EntryCommand::Missing { written, tried } => (
                    written.clone(),
                    Some(unrunnable_reason(&id, &written, &tried, &path)),
                ),
            };
            let hook = Hook {
                matcher: matcher.clone(),
                command,
            };
            if !tool.is_empty() && !hook_matches(&hook, tool) {
                continue;
            }

            let blocking = entry
                .get("blocking")
                .and_then(Value::as_bool)
                .unwrap_or(event_blocking);
            out.push(TamaHook {
                id,
                hook,

                blocking,
                unrunnable,
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
    let failed = outcome.exit_code != PASS_EXIT;
    if blocking && !approve && (failed || block_verdict) {
        // Exit code 2 is jeden's block signal; stdout/stderr carry the reason.
        HookOutcome {
            exit_code: BLOCK_EXIT,
            ..outcome
        }
    } else if !blocking
        && event == crate::hooks::event::PRE_TOOL_USE
        && (outcome.exit_code == BLOCK_EXIT || block_verdict)
    {
        HookOutcome {
            exit_code: PASS_EXIT,
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
/// that has at least one executable hook, and a row naming every registration
/// whose file is absent, because a guard that cannot start is the operator's
/// business.
pub fn describe_source(cwd: &Path) -> Option<String> {
    let path = registry_path(cwd)?;
    let registry = read_config(&path);
    let catalog = registry.get("catalog").cloned().unwrap_or(Value::Null);
    let events = registry.get("events").and_then(Value::as_object)?;
    let mut rows = Vec::new();
    let mut broken = Vec::new();
    for (tama_event, spec) in events {
        let Some((event, matcher)) = map_event(tama_event) else {
            continue;
        };
        let entries = spec
            .get("hooks")
            .and_then(Value::as_array)
            .map(Vec::as_slice)
            .unwrap_or_default();
        let mut count = 0;
        for entry in entries {
            match entry_command(entry, &path, &catalog) {
                Some(EntryCommand::Runnable(_)) => count += 1,
                Some(EntryCommand::Missing { written, .. }) => {
                    count += 1;
                    let id = entry.get("id").and_then(Value::as_str).unwrap_or("");
                    broken.push(format!("  {tama_event} -> {id}: no file at `{written}`"));
                }
                None => {}
            }
        }
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
    let mut text = format!(
        "Tama registry ({}, managedBy {}):\n{}",
        path.display(),
        managed_by,
        rows.join("\n")
    );
    if !broken.is_empty() {
        text.push_str("\nRegistrations with no executable here:\n");
        text.push_str(&broken.join("\n"));
    }
    Some(text)
}
