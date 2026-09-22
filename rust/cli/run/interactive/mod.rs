//! Interactive REPL loop and shared run-turn bookkeeping.

use parking_lot::Mutex;
use serde_json::{json, Value};
use std::env;
use std::fs;
use std::io::IsTerminal;
use std::path::Path;
use std::process::Command;
use std::sync::Arc;

use super::{run_turn_shared, self_rebuild};
use crate::cli::commands::expand::resolve_file_command;
use crate::cli::config::communication::{CodeFilter, DisplayPolicy};
use crate::cli::config::load_config;
use crate::cli::run::slash::{handle_slash, is_builtin_slash};
use crate::cli::run::slash_ui::{interactive_view, model_picker};
use crate::cli::sessions::{session_conversation_turns, session_dir_for};
use crate::{agent, hooks, read_json, tui, Args};

mod commands;
mod status;
mod turn;

use status::{git_prompt_status, service_tier_prompt};
use turn::run_turn;

fn input_accepts_attachments(input: &str) -> bool {
    let trimmed = input.trim();
    if !trimmed.starts_with('/') {
        return true;
    }
    let (command, rest) = trimmed
        .split_once(char::is_whitespace)
        .unwrap_or((trimmed, ""));
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
    let model = args
        .model
        .clone()
        .or(config.model)
        .or_else(|| env::var("JEDEN_MODEL").ok())
        .or_else(|| env::var("MODEL").ok())
        .filter(|model| !model.trim().is_empty());
    let initial_model_picker = if model.is_none() {
        // Brama unconfigured: skip the startup picker so the REPL still opens;
        // the welcome tip points at /setup to connect a router.
        let endpoint = env::var("BRAMA_URL")
            .ok()
            .filter(|value| !value.trim().is_empty());
        let brama = crate::control_plane::brama::BramaClient::configured(
            endpoint,
            env::var("BRAMA_TOKEN").ok(),
        );
        if brama.health().available {
            Some(model_picker(&args.cwd, None, false)?)
        } else {
            None
        }
    } else {
        None
    };
    let initial_picker = crate::onboarding::initial_picker(&args.cwd)?.or(initial_model_picker);
    let session_model = Arc::new(Mutex::new(model));
    // One persistent conversation provides native cross-turn memory for the
    // entire interactive session. A self-rebuild seeds a fresh recorder with
    // the prior durable turns while regenerating the system prompt from the
    // rebuilt executable.
    let mut initial_conversation = agent::Conversation::new(&args.cwd)?;
    if let Some(session_path) = &args.resume_session {
        if !session_path.exists() {
            return Err(format!(
                "resume session not found after rebuild: {}",
                session_path.display()
            ));
        }
        let mut turns = session_conversation_turns(session_path)?;
        if turns
            .first()
            .and_then(|turn| turn.get("role"))
            .and_then(Value::as_str)
            == Some("system")
        {
            turns.remove(0);
        }
        let count = turns.len();
        initial_conversation.load_history(&args.cwd, turns, session_path)?;
        println!(
            "Resumed {} prior turn(s) from {} after rebuilding Jeden.",
            count,
            session_path.display()
        );
    }
    let conversation = Arc::new(Mutex::new(initial_conversation));
    let pending_relaunch = Arc::new(Mutex::new(None));
    let context_limit = env::var("JEDEN_CONTEXT_LIMIT")
        .ok()
        .and_then(|v| v.trim().parse::<usize>().ok())
        .filter(|v| *v != usize::default());
    // SessionStart hooks fire once when the interactive session opens.
    let session_banner = hooks::session_start(&args.cwd, args.allow_command);
    if !session_banner.trim().is_empty() {
        println!("{}", session_banner.trim());
    }

    // Shared working directory keeps /move changes synchronized across subsequent
    // turns, the status line, git prompt, and file-command resolution.
    let session_cwd = Arc::new(Mutex::new(args.cwd.clone()));
    let status_model = Arc::clone(&session_model);
    let status_conv = Arc::clone(&conversation);
    let status_cwd = Arc::clone(&session_cwd);
    let status = move || {
        let cwd = status_cwd.lock().clone();
        let (branch, dirty_count) = git_prompt_status(&cwd);
        // try_lock: never block the frame on an in-flight turn.
        let tokens = status_conv.try_lock().map(|c| c.approx_tokens());
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
            model: status_model
                .lock()
                .clone()
                .unwrap_or_else(|| "not selected".into()),
            service_tier: service_tier_prompt(&cwd),
            branch,
            dirty_count,
            context_percent: None,
            context_limit: context_limit_label,
            cost: None,
        }
    };

    let classify = |input: &str| {
        let trimmed = input.trim();
        let command = trimmed.split_whitespace().next().unwrap_or(trimmed);
        if command == "/compact" || command == "/handoff" {
            return tui::TurnKind::Background;
        }
        if command.starts_with('/') && !is_builtin_slash(command) {
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
    hooks::session_stop(&session_cwd.lock().clone(), args.allow_command);
    if let Some(plan) = pending_relaunch.lock().take() {
        self_rebuild::execute(plan)?;
    }
    Ok(String::new())
}
