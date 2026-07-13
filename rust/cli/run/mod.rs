//! Run subtree: slash routing, interactive loop, and self-update.

use parking_lot::Mutex;
use std::sync::Arc;

use crate::{agent, Args};

pub(crate) mod interactive;
pub(crate) mod slash;
pub(crate) mod slash_ui;
pub(crate) use crate::update;

/// Run one agent turn against the shared conversation and persist retry/session
/// bookkeeping, mirroring the CLI `run` path.
pub(crate) fn run_turn_shared(
    conversation: &Arc<Mutex<agent::Conversation>>,
    args: &Args,
    task: &str,
    attachments: &[crate::model_router::ModelAttachment],
    hooks: &mut agent::RunHooks,
) -> Result<String, String> {
    let mut conv = conversation.lock();
    let result = conv.run_turn(args, task, attachments, hooks);
    match &result {
        Ok(_) => {
            let _ = agent::update_task_outcome(&args.cwd, task, true);
            let _ = agent::update_last_session_path(&args.cwd, &conv.session_path());
        }
        Err(_) => {
            let _ = agent::update_task_outcome(&args.cwd, task, false);
        }
    }
    let mut text = result.map(|text| text.trim().to_string())?;
    // Loop mode: auto-resubmit until exhausted (bounded), same as the CLI path.
    // The iteration cap comes from agent::MAX_LOOP_ITERS; a range walks it with
    // no bare counter literal.
    for _ in u32::MIN..agent::MAX_LOOP_ITERS {
        let Some(loop_prompt) = agent::loop_next_prompt(&args.cwd, task) else {
            break;
        };
        match conv.run_turn(args, &loop_prompt, &[], hooks) {
            Ok(more) => {
                let _ = agent::update_last_session_path(&args.cwd, &conv.session_path());
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
