//! The slash commands that act on the session itself rather than sending
//! anything to the model.
//!
//! Split out of `cli/run/interactive.rs`, which had grown past the module line
//! cap.

use super::super::self_rebuild::{self, RelaunchPlan};
use super::super::run_turn_shared;
use crate::cli::run::slash::{handle_slash};
use crate::cli::run::slash_ui::{interactive_view, model_picker};
use crate::{agent, tui, Args};
use parking_lot::Mutex;
use std::path::PathBuf;
use std::sync::Arc;

/// `Ok(None)` when this is not one of these commands, so the caller keeps
/// looking rather than treating an unknown command as handled.
#[allow(clippy::too_many_arguments)]
pub(super) fn session_command(
    command: &str,
    rest: &str,
    input: &str,
    ctx: &tui::TurnCtx,
    run_args: &Args,
    hooks: &mut agent::RunHooks,
    attachments: &[crate::model_router::ModelAttachment],
    handler_model: &Arc<Mutex<Option<String>>>,
    handler_conv: &Arc<Mutex<agent::Conversation>>,
    handler_cwd: &Arc<Mutex<PathBuf>>,
    handler_relaunch: &Arc<Mutex<Option<RelaunchPlan>>>,
) -> Result<Option<tui::CommandOutcome>, String> {
    let result: Result<String, String> = match command {
                "/todo" if rest.trim() == "continue" => {
                    handler_conv.lock().continue_work(&run_args, hooks)
                }
                "/todo" => {
                    let session = handler_conv.lock().session_path();
                    agent::update_last_session_path(&run_args.cwd, &session)?;
                    crate::completion::cli::execute(
                        &run_args.cwd,
                        &crate::slash::common::split_args(rest),
                        false,
                        Some(&run_args),
                    )
                }
                "/model" | "/models" | "/switch" => {
                    let next = rest.trim();
                    if matches!(next, "--all" | "-a") {
                        return model_picker(&run_args.cwd, handler_model.lock().as_deref(), true)
                            .map(|picker| Some(tui::CommandOutcome::Picker(picker)));
                    }
                    if next.is_empty() {
                        // Picker rows dispatch bare `/model` to reopen the curated list.
                        if ctx.from_view {
                            return model_picker(
                                &run_args.cwd,
                                handler_model.lock().as_deref(),
                                false,
                            )
                            .map(|picker| Some(tui::CommandOutcome::Picker(picker)));
                        }
                        return Ok(Some(tui::CommandOutcome::text(format!(
                            "Current model route: {}.",
                            handler_model.lock().as_deref().unwrap_or("not selected")
                        ))));
                    }
                    let message = crate::cli::run::slash::set_model_route(&run_args.cwd, next)?;
                    *handler_model.lock() = Some(next.to_string());
                    Ok(message)
                }
                "/setup" => {
                    let outcome = crate::slash::setup::run_interactive(
                        &run_args.cwd,
                        rest,
                        handler_model.lock().as_deref(),
                        true,
                    )?;
                    if rest
                        .split_whitespace()
                        .next()
                        .is_some_and(|verb| verb.eq_ignore_ascii_case("workspace"))
                    {
                        let selected =
                            crate::cli::workspace::configured_path()?.ok_or_else(|| {
                                "workspace adoption did not persist a path".to_string()
                            })?;
                        handler_conv.lock().rebase(&selected)?;
                        *handler_cwd.lock() = selected;
                    }
                    return Ok(Some(outcome));
                }
                "/onboarding" => {
                    let outcome = crate::onboarding::interactive(rest, &run_args.cwd)?;
                    if rest.split_whitespace().next() == Some("adopt-workspace") {
                        let selected =
                            crate::cli::workspace::configured_path()?.ok_or_else(|| {
                                "workspace adoption did not persist a path".to_string()
                            })?;
                        handler_conv.lock().rebase(&selected)?;
                        *handler_cwd.lock() = selected;
                    }
                    return Ok(Some(outcome));
                }
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
                        Some(view) => view.map(Some),
                        None if target.is_empty() => {
                            handle_slash(&run_args.cwd, input, handler_model.lock().as_deref())
                                .map(|text| Some(tui::CommandOutcome::text(text)))
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
                    return Ok(Some(tui::CommandOutcome::Exit(
                        "Rebuilt and health-checked Jeden; resuming this session in the new executable."
                            .into(),
                    )));
                }
                "/retry" => {
                    let task = agent::retry_task(&run_args)?;
                    run_turn_shared(&handler_conv, &run_args, &task, &attachments, hooks)
                }
                "/btw" => {
                    let task = agent::btw_task(rest)?;
                    run_turn_shared(&handler_conv, &run_args, &task, &attachments, hooks)
                }
                "/login" => {
                    let target = rest.trim();
                    if target.is_empty() {
                        return Ok(Some(tui::CommandOutcome::text(
                            crate::cli::auth::format_auth_status(&run_args.cwd),
                        )));
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
                "/compact" => handler_conv.lock().compact(&run_args, rest, hooks),
                "/handoff" => handler_conv.lock().handoff(&run_args, rest, hooks),
                "/checkpoint" => {
                    if rest.trim() == "list" {
                        handler_conv.lock().list_checkpoints()
                    } else {
                        handler_conv.lock().checkpoint(rest)
                    }
                }
                "/rewind" => handler_conv.lock().rewind(rest),
        _ => return Ok(None),
    };
    result.map(|text| Some(tui::CommandOutcome::text(text)))
}
