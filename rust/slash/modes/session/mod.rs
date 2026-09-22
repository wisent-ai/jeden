//! The session-shaped views: which roles are in force, which sessions exist,
//! and what starting, resuming or ending one does.

use crate::slash::common::split_head;
use crate::slash::state::{read_mode_state, ModeState};
use crate::slash::SlashContext;
use crate::tui::{PickerItem, PickerSpec};

mod dates;
mod listing;

use listing::{list_sessions, session_items, session_path};

/// Roles hub: one row per model role, each opening the existing view that
/// owns the role (`/model`, `/fast`, `/advisor`). Details report live state.
pub(crate) fn roles_picker(state: &ModeState, context: &SlashContext<'_>) -> PickerSpec {
    let lang = crate::cli::i18n::lang_code(context.cwd);
    let model = context
        .model
        .map(str::to_string)
        .or_else(|| crate::load_config(context.cwd).model)
        .or_else(|| std::env::var("JEDEN_MODEL").ok())
        .filter(|model| !model.trim().is_empty());
    let model_detail = match &model {
        Some(route) => format!("Current route: {route} · /model to change"),
        None => "No model route selected · /model to choose".to_string(),
    };
    let tier = if state.fast.service_tier.trim().is_empty() {
        "priority"
    } else {
        state.fast.service_tier.trim()
    };
    let fast_detail = if state.fast.enabled {
        format!("Enabled · service tier: {tier} · /fast to configure")
    } else {
        "Disabled · /fast to configure".to_string()
    };
    let advisor = &state.advisor;
    let advisor_detail = if advisor.enabled {
        format!(
            "Enabled · reviewer route: {} · /advisor to configure",
            super::advisor_model_label(advisor, context)
        )
    } else {
        "Disabled · /advisor to configure".to_string()
    };
    PickerSpec::new(
        crate::cli::i18n::tr(&lang, "view.roles.title"),
        vec![
            PickerItem::action("default model", "/roles model")
                .detail(model_detail)
                .badge("MODEL"),
            PickerItem::action("fast tier", "/roles fast")
                .detail(fast_detail)
                .badge(if state.fast.enabled { "ON" } else { "OFF" }),
            PickerItem::action("advisor", "/roles advisor")
                .detail(advisor_detail)
                .badge(if advisor.enabled { "ON" } else { "OFF" }),
        ],
    )
    .localized(&lang)
}

/// Non-interactive `/roles`: the same hub rows rendered as text.
pub(crate) fn handle_roles(context: &SlashContext<'_>) -> Result<String, String> {
    let state = read_mode_state(context.cwd);
    Ok(crate::tui::CommandOutcome::Picker(roles_picker(&state, context)).into_text())
}

pub(crate) fn advisor_picker(state: &ModeState, context: &SlashContext<'_>) -> PickerSpec {
    let lang = crate::cli::i18n::lang_code(context.cwd);
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
                .badge(crate::cli::i18n::tr(&lang, "badge.available")),
        );
        items.push(
            PickerItem::action("Show raw advisor notes", "/advisor dump raw")
                .detail("Display the complete reviewer response as JSON"),
        );
    }
    PickerSpec::new("Advisor workflow", items)
}

pub(crate) fn approval_picker(state: &ModeState, lang: &str) -> PickerSpec {
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
            .badge(if mode == current {
                crate::cli::i18n::tr(lang, "badge.current")
            } else {
                "MODE"
            })
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
                .badge(if policy == active {
                    crate::cli::i18n::tr(lang, "badge.current")
                } else {
                    "TOOL"
                })
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

pub(crate) fn tree_picker(state: &ModeState, lang: &str) -> PickerSpec {
    let mut items = vec![PickerItem::action("Show branch tree", "/tree show")
        .detail(if state.branches.is_empty() {
            "No recorded branches"
        } else {
            "Show every recorded branch"
        })
        .badge(if state.branches.is_empty() {
            "EMPTY"
        } else {
            crate::cli::i18n::tr(lang, "badge.available")
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

pub(crate) fn session_picker(context: &SlashContext<'_>) -> PickerSpec {
    let lang = crate::cli::i18n::lang_code(context.cwd);
    let mut items = vec![
        PickerItem::action("Show current session info", "/session info")
            .detail(format!("Workspace: {}", context.cwd.display()))
            .badge(crate::cli::i18n::tr(&lang, "badge.current")),
    ];
    items.extend(session_items(context.session_root));
    PickerSpec::new(crate::cli::i18n::tr(&lang, "view.session.title"), items).localized(&lang)
}

pub(crate) fn lifecycle_picker(state: &ModeState, context: &SlashContext<'_>) -> PickerSpec {
    let lang = crate::cli::i18n::lang_code(context.cwd);
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
                crate::cli::i18n::tr(&lang, "badge.active")
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
