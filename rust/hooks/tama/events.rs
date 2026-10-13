//! Reading the registry for one jeden event, and adapting what its hooks
//! answer to jeden's own decision contract.

use serde_json::Value;
use std::path::Path;

use super::command::{entry_command, unrunnable_reason, EntryCommand};
use super::{map_event, registry_path, TamaHook, BLOCK_EXIT, PASS_EXIT};
use crate::hooks::{hook_matches, parse_hook_json, read_config, Hook, HookOutcome};

/// Registry hooks for jeden `event` (`PreToolUse`, `UserPromptSubmit`, …),
/// Filter by tool and, when dispatching SessionStart, the lifecycle source.
/// An absent source lists all registrations for diagnostics.
pub fn load_event_hooks(
    cwd: &Path,
    event: &str,
    tool: &str,
    source: Option<&str>,
) -> Vec<TamaHook> {
    let path = match registry_path(cwd) {
        Ok(Some(path)) => path,
        Ok(None) => return Vec::new(),
        // A registry named explicitly that does not exist answers one
        // blocking registration that cannot start, like an unreadable one.
        Err(reason) => {
            return vec![TamaHook {
                id: "tama-registry".to_string(),
                hook: Hook {
                    matcher: String::new(),
                    command: String::new(),
                },
                blocking: true,
                unrunnable: Some(reason),
            }]
        }
    };
    // A registry that cannot be read or is not JSON answers one blocking
    // registration that cannot start, naming the file and the cause; read as
    // empty, every Tama guard on this machine stopped without a word.
    let registry = match read_config(&path) {
        Ok(registry) => registry,
        Err(reason) => {
            return vec![TamaHook {
                id: path.display().to_string(),
                hook: Hook {
                    matcher: String::new(),
                    command: String::new(),
                },
                blocking: true,
                unrunnable: Some(reason),
            }]
        }
    };
    let catalog = registry.get("catalog").cloned().unwrap_or(Value::Null);
    // A registry with no `events` table registers nothing a guard could run
    // from; like an unreadable one, it answers a blocking registration that
    // names the file, not an empty list that stops every guard silently.
    let Some(events) = registry.get("events").and_then(Value::as_object) else {
        return vec![TamaHook {
            id: path.display().to_string(),
            hook: Hook {
                matcher: String::new(),
                command: String::new(),
            },
            blocking: true,
            unrunnable: Some(format!(
                "the Tama registry {} has no events table, so no Tama hook can run",
                path.display()
            )),
        }];
    };
    let mut out = Vec::new();
    for (tama_event, spec) in events {
        let Some((mapped, matcher)) = map_event(tama_event) else {
            continue;
        };
        if mapped != event {
            continue;
        }
        if event == crate::hooks::event::SESSION_START {
            if let Some(source) = source {
                let compact = source == "compact";
                if (tama_event == "session_start:compact") != compact {
                    continue;
                }
            }
        }
        let event_blocking = spec
            .get("blocking")
            .and_then(Value::as_bool)
            .unwrap_or(false);
        // An event without hooks registers nothing. One whose hooks are not a
        // list cannot be read, and is reported like any registration that
        // cannot start rather than dropped with its guards.
        let entries = match spec.get("hooks") {
            None | Some(Value::Null) => continue,
            Some(Value::Array(entries)) => entries,
            Some(other) => {
                out.push(TamaHook {
                    id: format!("{}#{tama_event}", path.display()),
                    hook: Hook {
                        matcher: matcher.clone(),
                        command: String::new(),
                    },
                    blocking: true,
                    unrunnable: Some(format!(
                        "the Tama registry {} declares {tama_event} hooks that are not a list: {other}",
                        path.display()
                    )),
                });
                continue;
            }
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

/// Adapt a Tama hook outcome to jeden's decision contract. jeden refuses on
/// exit code 2 or stdout JSON `{"decision":"block"}` (see `block_decision`),
/// for a tool at `PreToolUse` and for an answer at `Stop`; a Tama `blocking`
/// event treats any failure (non-zero exit, spawn error) as a block, with an
/// explicit JSON `{"decision":"approve"}` overriding. A non-blocking
/// `PreToolUse` hook only records, so its block signals are scrubbed before
/// they reach the tool decision.
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
    let path = match registry_path(cwd) {
        Ok(path) => path?,
        Err(reason) => {
            return Some(format!(
                "Tama registry unusable; every event is refused until it is repaired: {reason}"
            ))
        }
    };
    let registry = match read_config(&path) {
        Ok(registry) => registry,
        Err(reason) => {
            return Some(format!(
                "Tama registry unreadable; every event is refused until it is repaired: {reason}"
            ))
        }
    };
    let catalog = registry.get("catalog").cloned().unwrap_or(Value::Null);
    let events = registry.get("events").and_then(Value::as_object)?;
    let mut rows = Vec::new();
    let mut broken = Vec::new();
    // A registry event Jeden has no counterpart for runs none of its hooks
    // here; it is named, never left out of the summary without a word.
    let mut unmapped = Vec::new();
    for (tama_event, spec) in events {
        let Some((event, matcher)) = map_event(tama_event) else {
            let ids: Vec<&str> = spec
                .get("hooks")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
                .filter_map(|entry| entry.get("id").and_then(Value::as_str))
                .collect();
            unmapped.push(format!("  {tama_event}: {}", ids.join(", ")));
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
    if rows.is_empty() && unmapped.is_empty() {
        return None;
    }
    let managed_by = match registry.get("managedBy").and_then(Value::as_str) {
        Some(owner) => owner,
        None => "not declared",
    };
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
    if !unmapped.is_empty() {
        text.push_str(
            "\nRegistry events Jeden has no event for; these hooks do not run in Jeden:\n",
        );
        text.push_str(&unmapped.join("\n"));
    }
    Some(text)
}
