//! Waiting for a managed child: its exit, the operator's cancellation, and
//! the signals that end its process tree.

use super::{ProcessEvent, TerminationReason};
use crate::tool_runtime::runtime_ops::{
    platform::{ProcessSignal, ProcessTree},
    OperationContext,
};
use std::process::{Child, ExitStatus};
use std::sync::mpsc::{Receiver, Sender};
use std::thread;

/// Wait for the child to finish. The only thing that ends this early is the
/// operator cancelling the turn: a command that is still running is still
/// doing the work it was asked to do, whatever a clock says about it. The
/// child is waited for on its own thread; this one blocks on the events. A
/// first cancellation asks the process tree to terminate, a repeated one (the
/// operator cancelling again) kills it.
pub(super) fn wait_owned_process(
    child: &mut Child,
    process_tree: &mut dyn ProcessTree,
    context: &OperationContext<'_>,
    events: &Receiver<ProcessEvent>,
    events_tx: &Sender<ProcessEvent>,
    preserve_descendants: bool,
) -> Result<(ExitStatus, TerminationReason), String> {
    let wake = events_tx.clone();
    crate::tool_runtime::runtime_ops::wake_on_cancellation(move || {
        wake.send(ProcessEvent::Cancelled).is_ok()
    });
    let exit_tx = events_tx.clone();
    thread::scope(|scope| -> Result<(ExitStatus, TerminationReason), String> {
        scope.spawn(move || {
            let _ = exit_tx.send(ProcessEvent::Exited(child.wait()));
        });
        let mut reason = TerminationReason::Completed;
        let mut escalation = 0u8;
        if context.cancellation().is_cancelled() {
            reason = TerminationReason::Cancelled;
            escalation = escalate(process_tree, escalation)?;
        }
        loop {
            match events.recv() {
                Ok(ProcessEvent::Progress(progress)) => context.progress(progress),
                Ok(ProcessEvent::Exited(status)) => {
                    let status = status.map_err(|error| error.to_string())?;
                    if reason == TerminationReason::Cancelled {
                        process_tree
                            .signal(ProcessSignal::Kill)
                            .map_err(|error| error.to_string())?;
                    } else if !preserve_descendants {
                        cleanup_descendants(process_tree)?;
                    }
                    return Ok((status, reason));
                }
                Ok(ProcessEvent::Cancelled) => {
                    if context.cancellation().is_cancelled() {
                        reason = TerminationReason::Cancelled;
                        escalation = escalate(process_tree, escalation)?;
                    }
                }
                Err(_) => return Err("managed process event channel closed".into()),
            }
        }
    })
}

/// Terminate on the first cancellation, kill on any later one.
fn escalate(process_tree: &mut dyn ProcessTree, escalation: u8) -> Result<u8, String> {
    let signal = if escalation == 0 {
        ProcessSignal::Terminate
    } else {
        ProcessSignal::Kill
    };
    process_tree
        .signal(signal)
        .map_err(|error| error.to_string())?;
    Ok(escalation.saturating_add(1))
}

/// Descendants left behind by a finished command are killed with it. A group
/// that is already gone is no failure; a kill the platform refuses leaves
/// processes running that the command started, so it fails the command.
fn cleanup_descendants(process_tree: &mut dyn ProcessTree) -> Result<(), String> {
    process_tree.signal(ProcessSignal::Kill).map_err(|error| {
        format!("descendants of the finished command could not be killed: {error}")
    })
}
