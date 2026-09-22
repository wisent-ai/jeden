//! What one interactive turn does with what the operator typed.
//!
//! Split out of `cli/run/interactive.rs`, which had grown past the module line
//! cap.

use super::super::self_rebuild::RelaunchPlan;
use super::super::{run_turn_shared, self_rebuild};
use super::{commands, input_accepts_attachments, model_attachments};
use crate::cli::commands::expand::resolve_file_command;
use crate::cli::config::communication::{CodeFilter, DisplayPolicy};
use crate::cli::run::slash::{handle_slash, is_builtin_slash};
use crate::cli::run::slash_ui::interactive_view;
use crate::cli::sessions::session_dir_for;
use crate::{agent, hooks, tui, Args};
use parking_lot::Mutex;
use serde_json::{json, Value};
use std::env;
use std::fs;
use std::path::PathBuf;
use std::sync::Arc;

/// One turn, with the session state every turn reads afresh.
#[allow(clippy::too_many_arguments)]
pub(super) fn run_turn(
    input: &str,
    ctx: &tui::TurnCtx,
    args: &Args,
    handler_model: &Arc<Mutex<Option<String>>>,
    handler_conv: &Arc<Mutex<agent::Conversation>>,
    handler_cwd: &Arc<Mutex<PathBuf>>,
    handler_relaunch: &Arc<Mutex<Option<RelaunchPlan>>>,
    local_escape_enabled: bool,
) -> Result<tui::CommandOutcome, String> {
        let mut run_args = args.clone();
        run_args.command = "run".into();
        run_args.model = handler_model.lock().clone();
        run_args.json = false;
        run_args.cwd = handler_cwd.lock().clone();
        // Read per turn so `/settings set communication.mode …` or a save from
        // Jeden Desktop changes what the next turn shows.
        let policy = DisplayPolicy::for_cwd(&run_args.cwd);
        let code_filter = std::cell::RefCell::new((!policy.code).then(CodeFilter::default));
        let mut hooks = agent::RunHooks {
            cancel: ctx.cancel.clone(),
            interactive: ctx.interactive,
            progress: Box::new(|message: &str| (ctx.progress)(policy.note(message))),
            stream: Box::new(|piece: &str| {
                let text = match code_filter.borrow_mut().as_mut() {
                    Some(filter) => filter.push(piece),
                    None => piece.to_string(),
                };
                if !text.is_empty() {
                    (ctx.stream)(&text);
                }
            }),
            trace: Box::new(|event: &agent::TraceEvent<'_>| {
                let shown = match event {
                    agent::TraceEvent::ToolCall { .. } => policy.tool_call_detail(),
                    agent::TraceEvent::ToolResult { .. } => policy.tool_results,
                    agent::TraceEvent::Reasoning { .. } => policy.reasoning,
                    agent::TraceEvent::CompletionState { .. }
                    | agent::TraceEvent::Message { .. } => true,
                };
                if shown {
                    (ctx.trace)(event);
                }
            }),
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
            if let Some(outcome) = commands::session_command(
                command,
                rest,
                input,
                ctx,
                &run_args,
                &mut hooks,
                &attachments,
                handler_model,
                handler_conv,
                handler_cwd,
                handler_relaunch,
            )? {
                return Ok(outcome);
            }
            match command {
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
                    if !crate::completion::read_state(&dir)?.complete() {
                        return Err("Session has retained work; explicitly cancel its requests before dropping it.".into());
                    }
                    *handler_conv.lock() = agent::Conversation::new(&run_args.cwd)?;
                    fs::remove_dir_all(&dir).map_err(|error| error.to_string())?;
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
                    handler_conv
                        .lock()
                        .load_history(&run_args.cwd, turns, &dir)?;
                    Ok(format!(
                        "Resumed {} into this conversation ({} prior turns loaded).",
                        dir.display(),
                        count
                    ))
                }
                "/context" => {
                    let task = rest.trim();
                    if !task.is_empty() {
                        // `/context <task>` asks the advisor; bare `/context`
                        // stays the window report it has always been.
                        let cwd = handler_cwd.lock().clone();
                        let config = crate::load_config(&cwd);
                        let settings = crate::context::advisor::settings(&cwd, &config);
                        let request =
                            crate::context::advisor::Request::from_settings(task, &settings);
                        let advice = crate::context::advisor::recommend(&cwd, &config, &request);
                        return Ok(tui::CommandOutcome::text(
                            crate::context::advisor::render_text(&advice),
                        ));
                    }
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
                        .map(|text| policy.answer(text))
                    } else {
                        run_turn_shared(&handler_conv, &run_args, input, &attachments, &mut hooks)
                            .map(|text| policy.answer(text))
                    }
                }
            }
        } else {
            run_turn_shared(&handler_conv, &run_args, input, &attachments, &mut hooks)
                .map(|text| policy.answer(text))
        };
        if !input.trim_start().starts_with('/') && result.is_ok() {
            crate::onboarding::observe_successful_turn();
        }
        result.map(tui::CommandOutcome::text)
}
