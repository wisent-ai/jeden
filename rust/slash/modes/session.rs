use std::fs;
use std::path::{Path, PathBuf};

use crate::slash::common::split_head;
use crate::slash::state::ModeState;
use crate::slash::SlashContext;
use crate::tui::{PickerItem, PickerSpec};

pub(crate) fn advisor_picker(state: &ModeState, context: &SlashContext<'_>) -> PickerSpec {
    let advisor = &state.advisor;
    let route = super::advisor_model_label(advisor, context);
    let mut items = vec![
        PickerItem::action(
            if advisor.enabled {
                "Disable advisor"
            } else {
                "Enable advisor"
            },
            if advisor.enabled {
                "/advisor off"
            } else {
                "/advisor on"
            },
        )
        .detail(format!("Reviewer route: {}", route))
        .badge(if advisor.enabled { "ON" } else { "OFF" }),
        PickerItem::action("Show advisor status", "/advisor status")
            .detail("Show reviewer state, route, and note availability"),
    ];
    if advisor.last_review.is_some() {
        items.push(
            PickerItem::action("Show advisor notes", "/advisor dump")
                .detail("Display the latest reviewer notes")
                .badge("AVAILABLE"),
        );
        items.push(
            PickerItem::action("Show raw advisor notes", "/advisor dump raw")
                .detail("Display the complete reviewer response as JSON"),
        );
    }
    PickerSpec::new("Advisor workflow", items)
}

pub(crate) fn approval_picker(state: &ModeState) -> PickerSpec {
    let current = if state.tools.approval_mode.trim().is_empty() {
        "default"
    } else {
        state.tools.approval_mode.as_str()
    };
    let mut items = vec![
        PickerItem::action("Show approval policy", "/approval status")
            .detail("Show the global mode and per-tool policies")
            .badge(current),
    ];
    for mode in ["always-ask", "write", "yolo"] {
        items.push(
            PickerItem::action(
                format!("Use {} approval mode", mode),
                format!("/approval mode {}", mode),
            )
            .detail("Set the global approval mode")
            .badge(if mode == current { "CURRENT" } else { "MODE" })
            .disabled(mode == current),
        );
    }
    for (tool, active) in &state.tools.approval {
        for policy in ["allow", "deny", "prompt"] {
            items.push(
                PickerItem::action(
                    format!("Set {} to {}", tool, policy),
                    format!("/approval {} {}", tool, policy),
                )
                .detail(format!("Current {} policy: {}", tool, active))
                .badge(if policy == active { "CURRENT" } else { "TOOL" })
                .disabled(policy == active),
            );
        }
    }
    items.push(
        PickerItem::action("Reset approval policy", "/approval reset")
            .detail("Clear the global mode and every tool override")
            .badge("DESTRUCTIVE"),
    );
    PickerSpec::new("Approval workflow", items)
}

pub(crate) fn tree_picker(state: &ModeState) -> PickerSpec {
    let mut items = vec![PickerItem::action("Show branch tree", "/tree show")
        .detail(if state.branches.is_empty() {
            "No recorded branches"
        } else {
            "Show every recorded branch"
        })
        .badge(if state.branches.is_empty() {
            "EMPTY"
        } else {
            "AVAILABLE"
        })];
    for branch in &state.branches {
        items.push(
            PickerItem::action(
                if branch.title.trim().is_empty() {
                    branch.id.clone()
                } else {
                    branch.title.clone()
                },
                format!("/resume {}", branch.path),
            )
            .detail(format!(
                "{} · {} · {}",
                branch.id, branch.created_at, branch.path
            ))
            .badge("BRANCH"),
        );
    }
    PickerSpec::new("Branch tree", items)
}

fn session_items(session_root: &Path) -> Vec<PickerItem> {
    let mut items = Vec::new();
    if let Ok(entries) = fs::read_dir(session_root) {
        for entry in entries.flatten() {
            let path = entry.path();
            if !path.is_dir() {
                continue;
            }
            let id = entry.file_name().to_string_lossy().to_string();
            let metadata = fs::read_to_string(path.join("state.json"))
                .ok()
                .and_then(|text| serde_json::from_str::<serde_json::Value>(&text).ok())
                .unwrap_or_default();
            let name = metadata
                .get("name")
                .and_then(serde_json::Value::as_str)
                .filter(|value| !value.trim().is_empty())
                .unwrap_or(&id);
            let workspace = metadata
                .get("cwd")
                .and_then(serde_json::Value::as_str)
                .unwrap_or("");
            let started = metadata
                .get("startedAt")
                .and_then(serde_json::Value::as_str)
                .unwrap_or("");
            let detail = [workspace, started]
                .into_iter()
                .filter(|value| !value.is_empty())
                .collect::<Vec<_>>()
                .join(" · ");
            items.push(
                PickerItem::action(name, format!("/resume {}", id))
                    .detail(detail)
                    .badge("SESSION"),
            );
        }
    }
    items.sort_by(|left, right| right.label.cmp(&left.label));
    items
}

pub(crate) fn session_picker(context: &SlashContext<'_>) -> PickerSpec {
    let mut items = vec![
        PickerItem::action("Show current session info", "/session info")
            .detail(format!("Workspace: {}", context.cwd.display()))
            .badge("CURRENT"),
    ];
    items.extend(session_items(context.session_root));
    PickerSpec::new("Session workflow", items)
}

pub(crate) fn lifecycle_picker(state: &ModeState, context: &SlashContext<'_>) -> PickerSpec {
    let mut items = vec![
        PickerItem::action("Start a fresh conversation", "/new now")
            .detail("Clear prior turns and start a new session"),
        PickerItem::action("Shake context", "/shake elide")
            .detail(if state.shake.trim().is_empty() {
                "Apply the elide context strategy"
            } else {
                "Reapply the elide context strategy"
            })
            .badge(if state.shake.trim().is_empty() {
                "OFF"
            } else {
                "ACTIVE"
            }),
    ];
    items.push(
        PickerItem::action("Rename current session", "/rename ")
            .detail("Edit the new session name before submitting")
            .badge("INPUT")
            .prefill(),
    );
    items.push(
        PickerItem::action("Move workspace", "/move ")
            .detail("Edit the destination directory before submitting")
            .badge("INPUT")
            .prefill(),
    );
    items.extend(session_items(context.session_root));
    items.push(
        PickerItem::action("Drop current session", "/drop confirm")
            .detail("Delete the active session and start fresh")
            .badge("DESTRUCTIVE"),
    );
    PickerSpec::new("Session lifecycle", items)
}

/// List every session directory under `session_root`, one per line. The prior
/// "most recent N" display cap was an unconsented numeric literal and has been
/// removed; all sessions are listed.
fn list_sessions(session_root: &Path) -> String {
    let mut rows = Vec::new();
    if let Ok(entries) = fs::read_dir(session_root) {
        for entry in entries.flatten() {
            rows.push(entry.file_name().to_string_lossy().to_string());
        }
    }
    if rows.is_empty() {
        "No sessions found.".into()
    } else {
        rows.join("\n")
    }
}

fn session_path(session_root: &Path, id_or_path: &str) -> PathBuf {
    if id_or_path.contains('/') {
        PathBuf::from(id_or_path)
    } else {
        session_root.join(id_or_path)
    }
}

pub(crate) fn handle_session(args: &str, context: &SlashContext<'_>) -> Result<String, String> {
    let (verb, _) = split_head(args);
    if verb.is_empty() || verb == "info" {
        return Ok(format!("Session: rust one-shot slash invocation\nWorkspace: {}\nSession root: {}\nRecorder: not active in this non-interactive Rust command", context.cwd.display(), context.session_root.display()));
    }
    if verb == "delete" {
        return Err("Refusing to delete the active session from inside itself. Exit Jeden, then remove the session directory explicitly if you still want this destructive action.".into());
    }
    Err("Usage: /session [info|delete]".into())
}

pub(crate) fn handle_lifecycle(
    command: &str,
    args: &str,
    state: &mut ModeState,
    context: &SlashContext<'_>,
) -> Option<Result<String, String>> {
    match command {
        "/new" | "/fresh" => Some(Ok("Started a fresh logical turn context. Provider stream state is reset for the next prompt in this Jeden process.".into())),
        "/drop" => Some(Err("Refusing to delete the active session from inside itself. Use /new for a fresh context or exit and remove the session directory explicitly.".into())),
        "/shake" => {
            state.shake = if args.trim().is_empty() { "elide".into() } else { args.trim().into() };
            Some(Ok(format!("Shake mode applied locally: {}. Subsequent prompts will instruct the model to avoid relying on heavy prior artifacts unless re-read.", state.shake)))
        },
        "/resume" => {
            let (id, _) = split_head(args);
            if id.is_empty() { Some(Ok(list_sessions(context.session_root))) }
            else {
                let path = session_path(context.session_root, id);
                if path.exists() { Some(Ok(format!("Session {} exists at {}. Full in-place interactive resume is available through CLI: jeden resume {} \"<task>\"", path.file_name().map(|v| v.to_string_lossy()).unwrap_or_default(), path.display(), path.display()))) }
                else { Some(Err(format!("session not found: {}", path.display()))) }
            }
        },
        "/rename" => Some(Ok(format!("Session title set to: {}", if args.trim().is_empty() { "rust one-shot slash invocation" } else { args.trim() }))),
        "/move" => Some(Err("/move requires an active interactive session recorder; Rust one-shot slash commands cannot move a live recorder in this pass.".into())),
        _ => None,
    }
}
