//! The `/setup` verbs: the text handler for non-interactive callers and the
//! interactive handler that keeps the wizard open between steps.
//!
//! Split out of `slash/setup/mod.rs`, which had grown past the module line cap.

use std::path::Path;

use super::super::common::split_head;
use super::super::SlashContext;
use super::env_file::{save_agent_id, save_brama_url};
use super::report::{checklist_text, validate_text};
use super::{preferences_picker, setup_picker};
use crate::tui::CommandOutcome;

const USAGE: &str = "Usage: /setup [status|validate|workspace <path>|brama-url <url>|agent-id <id>|model|preferences]";

/// Text-mode handler (non-interactive callers such as `jeden run "/setup …"`
/// and the slash fallback). Bare `/setup` and `/setup status` print the
/// checklist without changing anything.
pub(crate) fn handle_text(args: &str, context: &SlashContext<'_>) -> Result<String, String> {
    let (verb, rest) = split_head(args);
    match verb.to_ascii_lowercase().as_str() {
        "" => Ok(checklist_text(context)),
        "status" => Ok(checklist_text(context)),
        "validate" => Ok(validate_text(context)),
        "workspace" => {
            let path = if rest.trim().is_empty() {
                context.cwd
            } else {
                Path::new(rest.trim())
            };
            Ok(crate::cli::workspace::adopt(path, context.cwd)?.text())
        }
        "brama-url" => save_brama_url(rest),
        "agent-id" => save_agent_id(rest),
        "model" => Ok(needs_tui(context)),
        "preferences" => Ok(needs_tui(context)),
        _ => Err(USAGE.into()),
    }
}

fn needs_tui(context: &SlashContext<'_>) -> String {
    format!(
        "That step needs the interactive TUI; run bare `jeden` and then /setup.\n\n{}",
        checklist_text(context)
    )
}

/// After a step is saved: re-open the wizard on the next missing step, or
/// answer the step's message to a non-interactive caller.
fn after_step(
    context: &SlashContext<'_>,
    interactive: bool,
    message: String,
) -> Result<CommandOutcome, String> {
    if interactive {
        setup_picker(context).map(CommandOutcome::Picker)
    } else {
        Ok(CommandOutcome::Text(message))
    }
}

/// Interactive handler for the TUI: step submissions keep the wizard sequence
/// alive by returning the next view instead of plain text.
pub(crate) fn run_interactive(
    cwd: &Path,
    args: &str,
    current_model: Option<&str>,
    interactive: bool,
) -> Result<CommandOutcome, String> {
    let session_root = crate::session_root();
    let context = SlashContext {
        cwd,
        model: current_model,
        session_root: &session_root,
    };
    let (verb, rest) = split_head(args);
    match verb.to_ascii_lowercase().as_str() {
        "" if interactive => setup_picker(&context).map(CommandOutcome::Picker),
        "" => Ok(CommandOutcome::Text(checklist_text(&context))),
        "status" => Ok(CommandOutcome::Text(checklist_text(&context))),
        "validate" => Ok(CommandOutcome::Text(validate_text(&context))),
        "workspace" => {
            let path = if rest.trim().is_empty() { cwd } else { Path::new(rest.trim()) };
            let message = crate::cli::workspace::adopt(path, cwd)?.text();
            after_step(&context, interactive, message)
        }
        "brama-url" => after_step(&context, interactive, save_brama_url(rest)?),
        "agent-id" => after_step(&context, interactive, save_agent_id(rest)?),
        "model" => crate::cli::run::slash_ui::model_picker(cwd, current_model, false)
            .map(CommandOutcome::Picker)
            .map_err(|error| {
                format!(
                    "{error}\nHint: fix the Brama connection (step 1 writes ~/.jeden/.env), then rerun /setup. Completed steps are kept."
                )
            }),
        "preferences" => Ok(CommandOutcome::Picker(preferences_picker(cwd))),
        _ => Err(USAGE.into()),
    }
}
