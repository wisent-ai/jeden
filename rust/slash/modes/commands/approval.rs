//! How much a session may do before it asks.
//!
//! Split out of `slash/modes/mod.rs`, which had grown past the module line cap.

use crate::agent::{ApprovalMode, ToolPolicy};
use crate::slash::common::split_args;
use crate::slash::state::ModeState;
use crate::slash::support::state::ToolsState;

fn status(state: &ModeState) -> String {
    let mode = if state.tools.approval_mode.trim().is_empty() {
        "default (safe always-ask unless --yolo or both allow flags are set)"
    } else {
        state.tools.approval_mode.as_str()
    };
    let mut lines = vec![format!("Approval mode: {}", mode)];
    if state.tools.approval.is_empty() {
        lines.push("Per-tool policies: none.".into());
    } else {
        lines.push("Per-tool policies:".into());
        for (tool, policy) in &state.tools.approval {
            lines.push(format!("- {}: {}", tool, policy));
        }
    }
    lines.join("\n")
}

fn usage() -> String {
    format!(
        "Usage: /approval [status] | mode <{}> | <tool> <{}> | reset",
        ApprovalMode::choices(),
        ToolPolicy::choices()
    )
}

pub(crate) fn handle_approval(args: &str, state: &mut ModeState) -> Result<String, String> {
    let argv = split_args(args);
    let (first, rest) = match argv.split_first() {
        None => return Ok(status(state)),
        Some((first, _)) if first == "status" => return Ok(status(state)),
        Some(split) => split,
    };
    match (first.as_str(), rest) {
        ("mode", [mode, ..]) => {
            let mode: ApprovalMode = crate::vocabulary::named(mode).map_err(|cause| {
                format!(
                    "{cause}\nUsage: /approval mode <{}>",
                    ApprovalMode::choices()
                )
            })?;
            state.tools.approval_mode = mode.name();
            Ok(format!("Approval mode set to {}.", mode.name()))
        }
        ("mode", []) => Err(format!(
            "Usage: /approval mode <{}>",
            ApprovalMode::choices()
        )),
        ("reset", _) => {
            state.tools = ToolsState::default();
            Ok("Approval policy reset.".into())
        }
        (tool, [policy, ..]) if !tool.is_empty() => {
            let policy: ToolPolicy = crate::vocabulary::named(policy)
                .map_err(|cause| format!("{cause}\n{}", usage()))?;
            state.tools.approval.insert(tool.to_string(), policy.name());
            Ok(format!(
                "Approval policy for {} set to {}.",
                tool,
                policy.name()
            ))
        }
        _ => Err(usage()),
    }
}
