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
use crate::cli::config::load_config;
use crate::cli::run::slash::{handle_slash, is_builtin_slash};
use crate::cli::run::slash_ui::{interactive_view, model_picker};
use crate::cli::sessions::{session_conversation_turns, session_dir_for};
use crate::{agent, hooks, read_json, tui, Args};

fn git_prompt_status(cwd: &Path) -> (Option<String>, usize) {
    let branch = Command::new("git")
        .arg("-C")
        .arg(cwd)
        .args(["branch", "--show-current"])
        .output()
        .ok()
        .filter(|output| output.status.success())
        .map(|output| String::from_utf8_lossy(&output.stdout).trim().to_string())
        .filter(|value| !value.is_empty());
    let dirty_count = Command::new("git")
        .arg("-C")
        .arg(cwd)
        .args(["status", "--porcelain"])
        .output()
        .ok()
        .filter(|output| output.status.success())
        .map(|output| String::from_utf8_lossy(&output.stdout).lines().count())
        .unwrap_or_default();
    (branch, dirty_count)
}

fn service_tier_prompt(cwd: &Path) -> String {
    let mode: Value = read_json(&cwd.join(".jeden/mode-state.json"));
    if mode
        .pointer("/fast/enabled")
        .and_then(Value::as_bool)
        .unwrap_or(false)
    {
        if let Some(tier) = mode
            .pointer("/fast/serviceTier")
            .and_then(Value::as_str)
            .filter(|value| !value.trim().is_empty())
        {
            return tier.to_string();
        }
    }
    env::var("JEDEN_SERVICE_TIER")
        .ok()
        .or_else(|| env::var("MODEL_SERVICE_TIER").ok())
        .filter(|value| !value.trim().is_empty())
        .unwrap_or_else(|| "default".into())
}

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
    let initial_picker = crate::onboarding::initial_picker()?.or(initial_model_picker);
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
        initial_conversation.load_history(&args.cwd, turns)?;
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
    let handler = move |input: &str, ctx: &tui::TurnCtx| -> Result<tui::CommandOutcome, String> {
        let mut run_args = args.clone();
        run_args.command = "run".into();
        run_args.model = handler_model.lock().clone();
        run_args.json = false;
        run_args.cwd = handler_cwd.lock().clone();
        let mut hooks = agent::RunHooks {
            cancel: ctx.cancel.clone(),
            interactive: ctx.interactive,
            progress: Box::new(|message: &str| (ctx.progress)(message)),
            stream: Box::new(|piece: &str| (ctx.stream)(piece)),
            ask_user: ctx.ask_user.map(|ask_user| {
                Box::new(move |question: &str, options: &[String]| ask_user(question, options))
                    as Box<dyn Fn(&str, &[String]) -> Result<String, String>>
            }),
            approve: Box::new(|tool: &str, detail: &str| (ctx.approve)(tool, detail)),
            goal_event: None,
        };

        if local_escape_enabled {
            let trimmed = input.trim_start();
            let escape = trimmed
                .strip_prefix('!')
                .map(|code| ("run_command", code))
                .or_else(|| trimmed.strip_prefix('$').map(|code| ("python_eval", code)));
            if let Some((tool, code)) = escape {
                let code = code.trim();
                if code.is_empty() {
                    let hint = if tool == "run_command" {
                        "Usage: ! <command> — run a shell command in cwd; output stays local (no model turn)."
                    } else {
                        "Usage: $ <code> — run Python via python3 in cwd; output stays local (no model turn)."
                    };
                    return Ok(tui::CommandOutcome::text(hint));
                }
                let text = handler_conv
                    .lock()
                    .local_tool_exec(&run_args, &hooks, tool, code)?;
                return Ok(tui::CommandOutcome::text(text));
            }
        }

        if !ctx.attachments.is_empty() && !input_accepts_attachments(input) {
            let command = input.split_whitespace().next().unwrap_or(input);
            return Err(format!(
                "attachments cannot be used with local command `{command}`; submit them with a model prompt"
            ));
        }
        let attachments = model_attachments(ctx.attachments)?;

        if !ctx.from_view {
            if let Some(view) =
                interactive_view(&run_args.cwd, input, handler_model.lock().as_deref())
            {
                return view;
            }
        }
        let result: Result<String, String> = if input.trim_start().starts_with('/') {
            let trimmed = input.trim();
            let (command, rest) = trimmed
                .split_once(char::is_whitespace)
                .unwrap_or((trimmed, ""));
            match command {
                "/model" | "/models" | "/switch" => {
                    let next = rest.trim();
                    if matches!(next, "--all" | "-a") {
                        return model_picker(&run_args.cwd, handler_model.lock().as_deref(), true)
                            .map(tui::CommandOutcome::Picker);
                    }
                    if next.is_empty() {
                        // Picker rows dispatch bare `/model` to reopen the curated list.
                        if ctx.from_view {
                            return model_picker(
                                &run_args.cwd,
                                handler_model.lock().as_deref(),
                                false,
                            )
                            .map(tui::CommandOutcome::Picker);
                        }
                        return Ok(tui::CommandOutcome::text(format!(
                            "Current model route: {}.",
                            handler_model.lock().as_deref().unwrap_or("not selected")
                        )));
                    }
                    let message = super::slash::set_model_route(&run_args.cwd, next)?;
                    *handler_model.lock() = Some(next.to_string());
                    Ok(message)
                }
                "/setup" => {
                    return crate::slash::setup::run_interactive(
                        &run_args.cwd,
                        rest,
                        handler_model.lock().as_deref(),
                        ctx.interactive,
                    );
                }
                "/onboarding" => return crate::onboarding::interactive(rest),
                "/roles" | "/role" => {
                    // Roles hub rows dispatch `/roles <role>`; the hub has no
                    // backend of its own, so forward to the role's existing
                    // view even when a picker submitted the command.
                    let target = rest.trim();
                    let view_input = if target.is_empty() {
                        "/roles".to_string()
                    } else {
                        format!("/{target}")
                    };
                    return match interactive_view(
                        &run_args.cwd,
                        &view_input,
                        handler_model.lock().as_deref(),
                    ) {
                        Some(view) => view,
                        None if target.is_empty() => {
                            handle_slash(&run_args.cwd, input, handler_model.lock().as_deref())
                                .map(tui::CommandOutcome::text)
                        }
                        None => Err(format!(
                            "Unknown role: {target}. Usage: /roles [model|fast|advisor]"
                        )),
                    };
                }
                "/rebuild" | "/self-rebuild" => {
                    if !rest.trim().is_empty() {
                        return Err("Usage: /rebuild".into());
                    }
                    let session_path = handler_conv.lock().session_path();
                    let plan = self_rebuild::prepare(&run_args, &session_path)?;
                    *handler_relaunch.lock() = Some(plan);
                    return Ok(tui::CommandOutcome::Exit(
                        "Rebuilt and health-checked Jeden; resuming this session in the new executable."
                            .into(),
                    ));
                }
                "/retry" => {
                    let task = agent::retry_task(&run_args)?;
                    run_turn_shared(&handler_conv, &run_args, &task, &attachments, &mut hooks)
                }
                "/btw" => {
                    let task = agent::btw_task(rest)?;
                    run_turn_shared(&handler_conv, &run_args, &task, &attachments, &mut hooks)
                }
                "/login" => {
                    let target = rest.trim();
                    if target.is_empty() {
                        return Ok(tui::CommandOutcome::text(
                            crate::cli::auth::format_auth_status(&run_args.cwd),
                        ));
                    }
                    // Device-code flow: stream the code+QR into the live region,
                    // status goes beside the skeleton, Esc cancels the Weles poll.
                    let bridge = crate::cli::auth::TurnBridge {
                        progress: ctx.progress,
                        stream: ctx.stream,
                        ask_user: ctx.ask_user,
                    };
                    let cancel = ctx.cancel.clone();
                    crate::cli::auth::start_login_with_bridge(
                        &run_args.cwd,
                        target,
                        &bridge,
                        &move || cancel.load(std::sync::atomic::Ordering::Relaxed),
                    )
                }
                "/compact" => handler_conv.lock().compact(&run_args, rest, &mut hooks),
                "/handoff" => handler_conv.lock().handoff(&run_args, rest, &mut hooks),
                "/checkpoint" => {
                    if rest.trim() == "list" {
                        handler_conv.lock().list_checkpoints()
                    } else {
                        handler_conv.lock().checkpoint(rest)
                    }
                }
                "/rewind" => handler_conv.lock().rewind(rest),
                "/clear" | "/new" | "/fresh" => {
                    handler_conv.lock().reset(&run_args.cwd)?;
                    Ok("Started a fresh conversation; prior turns cleared.".into())
                }
                "/fork" => {
                    let path = handler_conv.lock().fork(&run_args.cwd)?;
                    Ok(format!(
                        "Forked into a new session at {}; the current context continues there.",
                        path.display()
                    ))
                }
                "/branch" => {
                    let title = rest.trim();
                    let path = handler_conv.lock().branch(&run_args.cwd)?;
                    let id = agent::record_branch(&run_args.cwd, title, &path)?;
                    Ok(format!("Branch {} created at {}; the current context continues on this branch. List with /tree, switch with /resume {}.", id, path.display(), path.display()))
                }
                "/force" => {
                    let t = rest.trim();
                    let (tool, prompt) = match t.find(char::is_whitespace) {
                        Some(i) => (t[..i].to_string(), t[i..].trim().to_string()),
                        None => (t.to_string(), String::new()),
                    };
                    if !tool.is_empty() && !prompt.is_empty() {
                        agent::arm_force_tool(&run_args.cwd, &tool)?;
                        run_turn_shared(&handler_conv, &run_args, &prompt, &attachments, &mut hooks)
                    } else {
                        handle_slash(&run_args.cwd, input, handler_model.lock().as_deref())
                    }
                }
                "/rename" => {
                    let name = rest.trim();
                    if name.is_empty() {
                        return Err("Usage: /rename <name>".into());
                    }
                    let dir = handler_conv.lock().session_path();
                    let state_path = dir.join("state.json");
                    let mut state = read_json::<Value>(&state_path);
                    if !state.is_object() {
                        state = json!({});
                    }
                    state
                        .as_object_mut()
                        .expect("state object")
                        .insert("name".into(), json!(name));
                    fs::write(
                        &state_path,
                        serde_json::to_string_pretty(&state).map_err(|e| e.to_string())? + "\n",
                    )
                    .map_err(|e| e.to_string())?;
                    Ok(format!("Session renamed to \"{}\".", name))
                }
                "/drop" => {
                    let dir = handler_conv.lock().session_path();
                    let _ = fs::remove_dir_all(&dir);
                    handler_conv.lock().reset(&run_args.cwd)?;
                    Ok(format!(
                        "Dropped session {} and started a fresh conversation.",
                        dir.display()
                    ))
                }
                "/resume" => {
                    let target = rest.trim();
                    if target.is_empty() {
                        return Err("Usage: /resume <session-id-or-path>".into());
                    }
                    let dir = session_dir_for(target);
                    if !dir.exists() {
                        return Err(format!("session not found: {}", dir.display()));
                    }
                    let turns = session_conversation_turns(&dir)?;
                    let count = turns.len();
                    handler_conv.lock().load_history(&run_args.cwd, turns)?;
                    Ok(format!(
                        "Resumed {} into this conversation ({} prior turns loaded).",
                        dir.display(),
                        count
                    ))
                }
                "/context" => {
                    let conv = handler_conv.lock();
                    Ok(format!(
                        "Live conversation: {} message(s), ~{} tokens.{}",
                        conv.turn_len(),
                        conv.approx_tokens(),
                        match context_limit {
                            Some(limit) =>
                                format!(" Context limit: {} tokens (JEDEN_CONTEXT_LIMIT).", limit),
                            None => " No context limit set (JEDEN_CONTEXT_LIMIT unset).".into(),
                        }
                    ))
                }
                "/move" => {
                    let target = rest.trim();
                    if target.is_empty() {
                        return Err("Usage: /move <directory>".into());
                    }
                    let base = handler_cwd.lock().clone();
                    let candidate = {
                        let p = std::path::Path::new(target);
                        if p.is_absolute() {
                            p.to_path_buf()
                        } else {
                            base.join(p)
                        }
                    };
                    let resolved = candidate
                        .canonicalize()
                        .map_err(|e| format!("cannot move to {}: {}", candidate.display(), e))?;
                    if !resolved.is_dir() {
                        return Err(format!("not a directory: {}", resolved.display()));
                    }
                    handler_conv.lock().rebase(&resolved)?;
                    *handler_cwd.lock() = resolved.clone();
                    Ok(format!("Working directory moved to {}. Tools, git status, and file commands now resolve there.", resolved.display()))
                }
                _ => {
                    if is_builtin_slash(command) {
                        handle_slash(&run_args.cwd, input, handler_model.lock().as_deref())
                    } else if let Some(expanded) =
                        resolve_file_command(&run_args.cwd, command, rest)
                    {
                        run_turn_shared(
                            &handler_conv,
                            &run_args,
                            &expanded,
                            &attachments,
                            &mut hooks,
                        )
                    } else {
                        run_turn_shared(&handler_conv, &run_args, input, &attachments, &mut hooks)
                    }
                }
            }
        } else {
            run_turn_shared(&handler_conv, &run_args, input, &attachments, &mut hooks)
        };
        if !input.trim_start().starts_with('/') && result.is_ok() {
            crate::onboarding::observe_successful_turn();
        }
        result.map(tui::CommandOutcome::text)
    };

    tui::run_basic_loop(status, classify, handler, initial_picker).map_err(|e| e.to_string())?;
    hooks::session_stop(&session_cwd.lock().clone(), args.allow_command);
    if let Some(plan) = pending_relaunch.lock().take() {
        self_rebuild::execute(plan)?;
    }
    Ok(String::new())
}
