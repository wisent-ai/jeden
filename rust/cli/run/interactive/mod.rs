//! Interactive REPL loop and shared run-turn bookkeeping.

use parking_lot::Mutex;
use serde_json::Value;
use std::io::IsTerminal;
use std::sync::Arc;

use super::self_rebuild;
use crate::cli::config::load_config;
use crate::cli::run::slash::is_builtin_slash;
use crate::cli::run::slash_ui::model_picker;
use crate::cli::sessions::session_conversation_turns;
use crate::{agent, hooks, tui, Args};

mod commands;
pub(crate) mod status;
mod turn;

use status::{service_tier_prompt, GitWatch};
use turn::run_turn;

fn input_accepts_attachments(input: &str) -> bool {
    let trimmed = input.trim();
    if !trimmed.starts_with('/') {
        return true;
    }
    let (command, rest) = match trimmed.split_once(char::is_whitespace) {
        Some(parts) => parts,
        None => (trimmed, ""),
    };
    match command {
        "/retry" | "/btw" => true,
        "/force" => {
            let mut fields = rest.split_whitespace();
            fields.next().is_some() && fields.next().is_some()
        }
        _ => !is_builtin_slash(command),
    }
}

fn model_attachments(
    items: &[tui::Attachment],
) -> Result<Vec<crate::model_router::ModelAttachment>, String> {
    items
        .iter()
        .map(|item| match &item.kind {
            tui::AttachmentKind::Image { mime, .. } => {
                crate::model_router::ModelAttachment::image(mime.clone(), item.bytes())
                    .map_err(|error| format!("attachment `{}`: {error}", item.name))
            }
            tui::AttachmentKind::Text { .. } => {
                crate::model_router::ModelAttachment::text(item.bytes())
                    .map_err(|error| format!("attachment `{}`: {error}", item.name))
            }
            tui::AttachmentKind::Binary { mime } => Err(format!(
                "attachment `{}` from {:?} has unsupported binary type `{mime}`",
                item.name, item.source
            )),
        })
        .collect()
}

pub(crate) fn interactive(args: &Args) -> Result<String, String> {
    let config = load_config(&args.cwd);
    // The unrelated `MODEL` variable is no route Jeden reads.
    let model = crate::agent::selected_model(args.model.as_deref(), config.model.as_deref())?;
    let initial_model_picker = if model.is_none() {
        // Keep /setup reachable when a direct route variable is unreadable;
        // its picker reports the refusal without treating Brama as selected.
        if crate::agent::credential::direct_provider().is_none()
            && crate::agent::credential::direct_refusal().is_some()
        {
            None
        } else {
            let client = crate::control_plane::model_client()?;
            if client.health().available {
                Some(model_picker(&args.cwd, None, false)?)
            } else {
                None
            }
        }
    } else {
        None
    };
    let initial_picker = crate::onboarding::initial_picker(&args.cwd)?.or(initial_model_picker);
    let session_model = Arc::new(Mutex::new(model));
    // One persistent conversation provides native cross-turn memory for the
    // entire interactive session. `--resume-session` (a session id, as
    // `jeden sessions` lists it, or its directory; a self-rebuild and Oko's
    // adoption of a running Jeden both name one) seeds a fresh recorder with
    // the prior durable turns while regenerating the system prompt from this
    // executable.
    // A named session that is not there is refused before a new one is
    // recorded, so a mistyped id leaves no empty session behind.
    let resumed = agent::resumed_session(args);
    if let Some(session_path) = &resumed {
        // One that cannot be examined is refused with its cause, never
        // called missing.
        if !crate::tool_runtime::runtime_ops::platform::presence::present(session_path)? {
            return Err(format!(
                "--resume-session names no session: {} does not exist",
                session_path.display()
            ));
        }
    }
    let mut initial_conversation = agent::Conversation::new(&args.cwd)?;
    if let Some(session_path) = &resumed {
        let mut turns = session_conversation_turns(session_path)?;
        let has_system = turns
            .first()
            .and_then(|turn| turn.get("role"))
            .and_then(Value::as_str)
            == Some("system");
        let count = turns.len() - usize::from(has_system);
        if has_system {
            if let Some(first) = turns.first_mut() {
                *first = serde_json::json!({
                    "role": "system",
                    "content": agent::system_prompt_checked(&args.cwd)?
                });
            }
        }
        initial_conversation.load_history(&args.cwd, turns, session_path)?;
        println!(
            "Resumed {} prior turn(s) from {}.",
            count,
            session_path.display()
        );
    }
    let conversation = Arc::new(Mutex::new(initial_conversation));
    let pending_relaunch = Arc::new(Mutex::new(None));
    let context_limit = status::context_limit()?;

    // Shared working directory keeps /move changes synchronized across subsequent
    // turns, the status line, git prompt, and file-command resolution.
    let session_cwd = Arc::new(Mutex::new(args.cwd.clone()));
    let status_model = Arc::clone(&session_model);
    let status_conv = Arc::clone(&conversation);
    let status_cwd = Arc::clone(&session_cwd);
    let mut git_watch = GitWatch::default();
    let status = move || {
        let cwd = status_cwd.lock().clone();
        // try_lock: never block the frame on an in-flight turn.
        let tokens = status_conv.try_lock().map(|c| c.approx_tokens());
        let git = git_watch.current(&cwd, tokens);
        // Token counts (used/limit) are shown directly; no scaling constant.
        let context_limit_label = match (tokens, context_limit) {
            (Some(tokens), Some(limit)) => Some(format!("{}/{} tok", tokens, limit)),
            (Some(tokens), None) => Some(format!("~{} tok", tokens)),
            _ => None,
        };
        tui::PromptStatus {
            cwd: cwd.display().to_string(),
            write_status: if args.allow_write {
                "allow".into()
            } else {
                "ask".into()
            },
            command_status: if args.allow_command {
                "allow".into()
            } else {
                "ask".into()
            },
            model: match status_model.lock().clone() {
                Some(model) => model,
                None => "not selected".into(),
            },
            service_tier: service_tier_prompt(&cwd),
            branch: git.branch,
            dirty_count: git.dirty_count,
            context_percent: None,
            context_limit: context_limit_label,
            cost: None,
        }
    };

    let classify = |input: &str| {
        let command = input.trim().split_whitespace().next();
        if matches!(command, Some("/compact" | "/handoff")) {
            return tui::TurnKind::Background;
        }
        if command.is_some_and(|command| command.starts_with('/') && !is_builtin_slash(command)) {
            return tui::TurnKind::Background;
        }
        tui::default_turn_kind(input)
    };

    let handler_model = Arc::clone(&session_model);
    let handler_conv = Arc::clone(&conversation);
    let handler_cwd = Arc::clone(&session_cwd);
    let handler_relaunch = Arc::clone(&pending_relaunch);
    // `! <shell>` / `$ <python>` escapes are a TTY-only affordance; piped stdin
    // (script mode) keeps forwarding such lines to the model unchanged.
    let local_escape_enabled = std::io::stdin().is_terminal() && std::io::stdout().is_terminal();
    let args = args.clone();
    let handler = move |input: &str, ctx: &tui::TurnCtx| -> Result<tui::CommandOutcome, String> {
        run_turn(
            input,
            ctx,
            &args,
            &handler_model,
            &handler_conv,
            &handler_cwd,
            &handler_relaunch,
            local_escape_enabled,
        )
    };

    tui::run_basic_loop(status, classify, handler, initial_picker).map_err(|e| e.to_string())?;
    if let Some(plan) = pending_relaunch.lock().take() {
        self_rebuild::execute(plan)?;
    }
    Ok(String::new())
}
