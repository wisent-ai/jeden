//! How much a session may do before it asks.
//!
//! Split out of `slash/modes/mod.rs`, which had grown past the module line cap.

use crate::slash::common::split_args;
use crate::slash::state::ModeState;
use crate::slash::support::state::ToolsState;

fn valid_approval_mode(value: &str) -> bool {
    matches!(value, "always-ask" | "write" | "yolo")
}

fn valid_approval_policy(value: &str) -> bool {
    matches!(value, "allow" | "deny" | "prompt")
}

pub(crate) fn handle_approval(args: &str, state: &mut ModeState) -> Result<String, String> {
    let argv = split_args(args);
    if argv.is_empty() || argv.first().map(String::as_str) == Some("status") {
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
        return Ok(lines.join("\n"));
    }
    let (first, rest) = argv
        .split_first()
        .expect("argv is non-empty after the is_empty guard above");
    match first.as_str() {
        "mode" => {
            let mode = rest.first().map(String::as_str).unwrap_or("");
            if !valid_approval_mode(mode) {
                return Err("Usage: /approval mode <always-ask|write|yolo>".into());
            }
            state.tools.approval_mode = mode.to_string();
            Ok(format!("Approval mode set to {}.", mode))
        }
        "reset" => {
            state.tools = ToolsState::default();
            Ok("Approval policy reset.".into())
        }
        tool => {
            let policy = rest.first().map(String::as_str).unwrap_or("");
            if tool.is_empty() || !valid_approval_policy(policy) {
                return Err("Usage: /approval [status] | mode <always-ask|write|yolo> | <tool> <allow|deny|prompt> | reset".into());
            }
            state
                .tools
                .approval
                .insert(tool.to_string(), policy.to_string());
            Ok(format!("Approval policy for {} set to {}.", tool, policy))
        }
    }
}
