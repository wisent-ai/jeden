//! Reading the registry for one jeden event, and adapting what its hooks
//! answer to jeden's own decision contract.

mod registry;

use serde_json::Value;
use std::path::Path;

use super::{TamaHook, BLOCK_EXIT, PASS_EXIT};
use crate::hooks::{hook_matches, parse_hook_json, HookOutcome};
use registry::{refused, Registry};

/// The id a refusal of the whole registry is reported under.
const REGISTRY_ID: &str = "tama-registry";

/// Registry hooks for jeden `event` (`PreToolUse`, `UserPromptSubmit`, …),
/// filtered by tool and, when dispatching SessionStart, the lifecycle source.
/// An absent source lists all registrations for diagnostics. A registry that
/// cannot be used answers one blocking registration naming why.
pub fn load_event_hooks(
    cwd: &Path,
    event: &str,
    tool: &str,
    source: Option<&str>,
) -> Vec<TamaHook> {
    let registry = match Registry::resolve(cwd) {
        Ok(Some(registry)) => registry,
        Ok(None) => return Vec::new(),
        Err(reason) => return vec![refused(REGISTRY_ID.to_string(), String::new(), reason)],
    };
    let (declared, _) = registry.declared();
    let mut out = Vec::new();
    for declared in declared {
        if declared.event != event {
            continue;
        }
        if event == crate::hooks::event::SESSION_START {
            if let Some(source) = source {
                let compact = source == "compact";
                if (declared.tama_event == "session_start:compact") != compact {
                    continue;
                }
            }
        }
        out.extend(
            declared
                .hooks
                .into_iter()
                .filter(|tama| tool.is_empty() || hook_matches(&tama.hook, tool)),
        );
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
/// with at least one registration, a row naming every registration that
/// cannot run and why, and every registry event Jeden has no event for.
pub fn describe_source(cwd: &Path) -> Option<String> {
    let registry = match Registry::resolve(cwd) {
        Ok(registry) => registry?,
        Err(reason) => {
            return Some(format!(
                "Tama registry unusable; every event is refused until it is repaired: {reason}"
            ))
        }
    };
    let (declared, unmapped) = registry.declared();
    let mut rows = Vec::new();
    let mut broken = Vec::new();
    for declared in declared
        .iter()
        .filter(|declared| !declared.hooks.is_empty())
    {
        for hook in &declared.hooks {
            if let Some(reason) = &hook.unrunnable {
                broken.push(format!(
                    "  {} -> {}: {reason}",
                    declared.tama_event, hook.id
                ));
            }
        }
        let matcher = if declared.matcher.is_empty() {
            "*"
        } else {
            declared.matcher.as_str()
        };
        rows.push(format!(
            "  {} -> {} [{matcher}] {} hook(s){}",
            declared.tama_event,
            declared.event,
            declared.hooks.len(),
            if declared.blocking { ", blocking" } else { "" }
        ));
    }
    if rows.is_empty() && unmapped.is_empty() {
        return None;
    }
    let managed_by = match &registry.managed_by {
        Some(owner) => owner.as_str(),
        None => "not declared",
    };
    let mut text = format!(
        "Tama registry ({}, managedBy {managed_by}):\n{}",
        registry.path.display(),
        rows.join("\n")
    );
    if !broken.is_empty() {
        text.push_str("\nRegistrations that cannot run here:\n");
        text.push_str(&broken.join("\n"));
    }
    if !unmapped.is_empty() {
        let unmapped: Vec<String> = unmapped
            .iter()
            .map(|(event, ids)| format!("  {event}: {}", ids.join(", ")))
            .collect();
        text.push_str(
            "\nRegistry events Jeden has no event for; these hooks do not run in Jeden:\n",
        );
        text.push_str(&unmapped.join("\n"));
    }
    Some(text)
}
