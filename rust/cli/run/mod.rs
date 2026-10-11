//! Run subtree: slash routing, interactive loop, and self-update.

use parking_lot::Mutex;
use std::sync::Arc;

use crate::{agent, Args};

pub(crate) mod interactive;
pub(crate) mod self_rebuild;
pub(crate) mod slash;
pub(crate) mod slash_ui;
pub(crate) use crate::update;

/// Run one agent turn against the shared conversation and persist retry/session
/// bookkeeping, mirroring the CLI `run` path: a record that cannot be written
/// is the turn's error, naming the workspace and the cause, because `/retry`
/// and every session command read it; it was dropped without a word.
pub(crate) fn run_turn_shared(
    conversation: &Arc<Mutex<agent::Conversation>>,
    args: &Args,
    task: &str,
    attachments: &[crate::model_router::ModelAttachment],
    hooks: &mut agent::RunHooks,
) -> Result<String, String> {
    let mut conv = conversation.lock();
    let result = conv.run_turn(args, task, attachments, hooks);
    let workspace = args.cwd.display();
    let mut text = match result {
        Err(error) => {
            return Err(match agent::update_task_outcome(&args.cwd, task, false) {
                Ok(()) => error,
                Err(recording) => format!(
                    "{error}; cannot persist failed task outcome for workspace {workspace}: {recording}"
                ),
            });
        }
        Ok(text) => text.trim().to_string(),
    };
    agent::update_task_outcome(&args.cwd, task, true).map_err(|error| {
        format!("cannot persist successful task outcome for workspace {workspace}: {error}")
    })?;
    record_session(args, &conv)?;
    // Loop mode: auto-resubmit until the count or deadline `/loop` was given
    // runs out, `/loop off`, a failed turn or cancellation, as the CLI path does.
    while let Some(loop_prompt) = agent::loop_next_prompt(&args.cwd, task)? {
        if hooks.cancelled() {
            break;
        }
        match conv.run_turn(args, &loop_prompt, &[], hooks) {
            Ok(more) => {
                record_session(args, &conv)?;
                text = format!("{}\n\n— loop resubmit —\n{}", text, more.trim());
            }
            Err(error) => {
                text = format!("{}\n\n— loop resubmit failed —\n{}", text, error);
                break;
            }
        }
    }
    Ok(text)
}

/// Record the conversation's session as the workspace's last one, or say why
/// it could not be recorded.
fn record_session(args: &Args, conv: &agent::Conversation) -> Result<(), String> {
    let session = conv.session_path();
    agent::update_last_session_path(&args.cwd, &session).map_err(|error| {
        format!(
            "cannot persist last session {} for workspace {}: {error}",
            session.display(),
            args.cwd.display()
        )
    })
}
