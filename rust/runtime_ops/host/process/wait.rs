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
    let pid = child.id();
    thread::scope(|scope| -> Result<(ExitStatus, TerminationReason), String> {
        scope.spawn(move || {
            let exited = stado_wait::blocking(
                stado_wait::Kind::Process,
                "a managed command to exit",
                format!("process {pid}"),
                || child.wait(),
            );
            // The waiter below has returned when no one receives the exit.
            let _unreceived = exit_tx.send(ProcessEvent::Exited(exited));
        });
        let mut reason = TerminationReason::Completed;
        // The first cancellation terminates the tree; any later one kills it.
        let mut next_signal = ProcessSignal::Terminate;
        if context.cancellation().is_cancelled() {
            reason = TerminationReason::Cancelled;
            next_signal = escalate(process_tree, next_signal)?;
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
                        next_signal = escalate(process_tree, next_signal)?;
                    }
                }
                Err(closed) => {
                    return Err(format!("managed process event channel closed: {closed}"))
                }
            }
        }
    })
}

/// Send `signal` to the tree; whatever is sent, a later cancellation kills.
fn escalate(
    process_tree: &mut dyn ProcessTree,
    signal: ProcessSignal,
) -> Result<ProcessSignal, String> {
    process_tree
        .signal(signal)
        .map_err(|error| error.to_string())?;
    Ok(ProcessSignal::Kill)
}

/// Descendants left behind by a finished command are killed with it. A group
/// that is already gone is no failure; a kill the platform refuses leaves
/// processes running that the command started, so it fails the command.
fn cleanup_descendants(process_tree: &mut dyn ProcessTree) -> Result<(), String> {
    process_tree.signal(ProcessSignal::Kill).map_err(|error| {
        format!("descendants of the finished command could not be killed: {error}")
    })
}
