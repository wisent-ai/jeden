//! Running one command as a child of this process, under the authority and
//! the limits it was granted.

use super::super::{
    platform::{native, ProcessSignal, ProcessTree},
    OperationContext, OperationProgress, OutputCapture,
};
use std::io::Write;
use std::process::{Child, ExitStatus, Stdio};
use std::sync::mpsc::{self, Receiver, Sender};
use std::thread;

mod capture;
mod command;
mod limits;

use crate::tool_runtime::runtime_ops::output::ArtifactSink;
use crate::tool_runtime::runtime_ops::output::OutputLimits;
use crate::tool_runtime::runtime_ops::security::ResourceLimits;
use capture::{capture_stream, drain_progress};
pub use command::ManagedCommand;
use command::ManagedStdio;
use limits::configure_resource_limits;
use std::io;

/// What the wait for a child reacts to: output progress from the capture
/// threads, the child's exit from the waiter thread, and an announced
/// cancellation.
pub(super) enum ProcessEvent {
    Progress(OperationProgress),
    Exited(io::Result<ExitStatus>),
    Cancelled,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TerminationReason {
    Completed,
    Cancelled,
}

#[derive(Debug)]
pub struct ManagedProcessResult {
    pub status: ExitStatus,
    pub reason: TerminationReason,
    pub stdout: OutputCapture,
    pub stderr: OutputCapture,
}

#[derive(Clone, Copy, Debug, Default)]
pub struct ProcessManager;

impl ProcessManager {
    pub fn run(
        &self,
        context: &OperationContext<'_>,
        command: ManagedCommand,
    ) -> Result<ManagedProcessResult, String> {
        let grant = context.execution_grant();
        super::SecureRuntime::detect()
            .authorize(grant)
            .map_err(|error| error.to_string())?;
        if !grant.permits_program(&command.program) {
            return Err(super::GrantError::ProgramDenied(
                command.program.to_string_lossy().into_owned(),
            )
            .to_string());
        }
        let cwd = command
            .cwd
            .canonicalize()
            .map_err(|error| format!("process cwd unavailable: {error}"))?;
        if !grant
            .filesystem
            .read_roots
            .iter()
            .any(|root| cwd.starts_with(root))
        {
            return Err(super::GrantError::FilesystemDenied(format!(
                "process cwd {} is outside grant",
                cwd.display()
            ))
            .to_string());
        }
        if command.stdio == ManagedStdio::InheritedForeground && !grant.process.inherit_stdio {
            return Err("process inherited stdio denied by execution grant".into());
        }
        let mut builder = super::super::sandbox::command(&command.program, grant)
            .map_err(|error| error.to_string())?;
        builder.env_clear();
        for key in &grant.process.environment {
            if let Some(value) = std::env::var_os(key) {
                builder.env(key, value);
            }
        }
        builder
            .args(&command.args)
            .current_dir(&command.cwd)
            .stdin(if command.stdio == ManagedStdio::InheritedForeground {
                Stdio::inherit()
            } else if command.stdin.is_some() {
                Stdio::piped()
            } else {
                Stdio::null()
            })
            .stdout(if command.stdio == ManagedStdio::InheritedForeground {
                Stdio::inherit()
            } else {
                Stdio::piped()
            })
            .stderr(if command.stdio == ManagedStdio::InheritedForeground {
                Stdio::inherit()
            } else {
                Stdio::piped()
            });
        for (key, _) in &command.env {
            if !grant
                .process
                .environment
                .contains(&key.to_string_lossy().into_owned())
            {
                return Err(format!(
                    "process environment variable {} denied",
                    key.to_string_lossy()
                ));
            }
        }
        for (key, value) in &command.env {
            if let Some(value) = value {
                builder.env(key, value);
            } else {
                builder.env_remove(key);
            }
        }
        native()
            .configure_command(&mut builder)
            .map_err(|error| error.to_string())?;
        configure_resource_limits(&mut builder, grant.resource_limits)?;
        let mut child = builder.spawn().map_err(|error| error.to_string())?;
        let mut process_tree = match native().attach_process_tree(&child) {
            Ok(tree) => tree,
            Err(error) => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(error.to_string());
            }
        };
        let preserve_descendants = command.preserve_descendants;
        if command.stdio == ManagedStdio::InheritedForeground {
            let (events_tx, events_rx) = mpsc::channel();
            let (status, reason) = wait_owned_process(
                &mut child,
                process_tree.as_mut(),
                context,
                &events_rx,
                &events_tx,
                preserve_descendants,
            )?;
            return Ok(ManagedProcessResult {
                status,
                reason,
                stdout: OutputCapture::uncaptured(),
                stderr: OutputCapture::uncaptured(),
            });
        }
        let stdout = child
            .stdout
            .take()
            .ok_or("managed process stdout unavailable")?;
        let stderr = child
            .stderr
            .take()
            .ok_or("managed process stderr unavailable")?;
        let stdin = child.stdin.take();
        let limits = context.output_limits();
        let artifacts = context.artifacts().clone();
        let (progress_tx, progress_rx) = mpsc::channel();

        thread::scope(|scope| -> Result<ManagedProcessResult, String> {
            let stdout_tx = progress_tx.clone();
            let stdout_artifacts = artifacts.clone();
            let stdout_reader = scope.spawn(move || {
                capture_stream("stdout", stdout, limits, stdout_artifacts, stdout_tx)
            });
            let stderr_tx = progress_tx.clone();
            let stderr_reader =
                scope.spawn(move || capture_stream("stderr", stderr, limits, artifacts, stderr_tx));
            let stdin_writer = scope.spawn(move || -> io::Result<()> {
                if let (Some(mut pipe), Some(bytes)) = (stdin, command.stdin) {
                    pipe.write_all(&bytes)?;
                }
                Ok(())
            });

            let (status, reason) = wait_owned_process(
                &mut child,
                process_tree.as_mut(),
                context,
                &progress_rx,
                &progress_tx,
                preserve_descendants,
            )?;
            drop(progress_tx);
            let stdin_result = stdin_writer
                .join()
                .map_err(|_| "managed process stdin writer panicked".to_string())?;
            if let Err(error) = stdin_result {
                if reason == TerminationReason::Completed {
                    return Err(format!("failed writing process stdin: {error}"));
                }
            }
            let stdout = stdout_reader
                .join()
                .map_err(|_| "managed stdout reader panicked".to_string())??;
            let stderr = stderr_reader
                .join()
                .map_err(|_| "managed stderr reader panicked".to_string())??;
            drain_progress(context, &progress_rx);
            Ok(ManagedProcessResult {
                status,
                reason,
                stdout,
                stderr,
            })
        })
    }
}

/// Wait for the child to finish. The only thing that ends this early is the
/// operator cancelling the turn: a command that is still running is still
/// doing the work it was asked to do, whatever a clock says about it. The
/// child is waited for on its own thread; this one blocks on the events. A
/// first cancellation asks the process tree to terminate, a repeated one (the
/// operator cancelling again) kills it.
fn wait_owned_process(
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
                        cleanup_descendants(process_tree);
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

/// Descendants left behind by a finished command are killed with it.
fn cleanup_descendants(process_tree: &mut dyn ProcessTree) {
    let _ = process_tree.signal(ProcessSignal::Kill);
}
