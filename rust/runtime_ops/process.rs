use super::{BoundedOutput, OperationContext, OperationProgress, OutputCapture};
use std::ffi::OsString;
use std::io::{self, Read, Write};
use std::path::PathBuf;
use std::process::{Child, Command, ExitStatus, Stdio};
use std::thread;
use std::sync::mpsc::{self, Receiver, Sender};
use std::time::{Duration, Instant};

const POLL_INTERVAL: Duration = Duration::from_millis(20);
const TERMINATION_GRACE: Duration = Duration::from_millis(500);

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum ManagedStdio {
    Captured,
    InheritedForeground,
}

#[derive(Clone, Debug)]
pub struct ManagedCommand {
    pub program: OsString,
    pub args: Vec<OsString>,
    pub cwd: PathBuf,
    pub env: Vec<(OsString, Option<OsString>)>,
    pub stdin: Option<Vec<u8>>,
    stdio: ManagedStdio,
}

impl ManagedCommand {
    pub fn new(program: impl Into<OsString>, cwd: impl Into<PathBuf>) -> Self {
        Self {
            program: program.into(),
            args: Vec::new(),
            cwd: cwd.into(),
            env: Vec::new(),
            stdin: None,
            stdio: ManagedStdio::Captured,
        }
    }

    pub(crate) fn inherit_stdio_for_foreground(&mut self) {
        self.stdio = ManagedStdio::InheritedForeground;
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TerminationReason {
    Completed,
    Cancelled,
    TimedOut,
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
        timeout: Duration,
    ) -> Result<ManagedProcessResult, String> {
        let mut builder = Command::new(&command.program);
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
            .stdout(if command.stdio == ManagedStdio::InheritedForeground { Stdio::inherit() } else { Stdio::piped() })
            .stderr(if command.stdio == ManagedStdio::InheritedForeground { Stdio::inherit() } else { Stdio::piped() });
        for (key, value) in &command.env {
            if let Some(value) = value {
                builder.env(key, value);
            } else {
                builder.env_remove(key);
            }
        }
        configure_process_group(&mut builder);
        let mut child = builder.spawn().map_err(|error| error.to_string())?;
        let group = child.id();
        if command.stdio == ManagedStdio::InheritedForeground {
            let deadline = context.effective_deadline(timeout);
            let (_progress_tx, progress_rx) = mpsc::channel();
            let (status, reason) = wait_owned_process(
                &mut child,
                group,
                deadline,
                context,
                &progress_rx,
            )?;
            return Ok(ManagedProcessResult {
                status,
                reason,
                stdout: OutputCapture::uncaptured(),
                stderr: OutputCapture::uncaptured(),
            });
        }
        let stdout = child.stdout.take().ok_or("managed process stdout unavailable")?;
        let stderr = child.stderr.take().ok_or("managed process stderr unavailable")?;
        let stdin = child.stdin.take();
        let deadline = context.effective_deadline(timeout);
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
            let stderr_reader = scope.spawn(move || {
                capture_stream("stderr", stderr, limits, artifacts, stderr_tx)
            });
            drop(progress_tx);
            let stdin_writer = scope.spawn(move || -> io::Result<()> {
                if let (Some(mut pipe), Some(bytes)) = (stdin, command.stdin) {
                    pipe.write_all(&bytes)?;
                }
                Ok(())
            });

            let (status, reason) = wait_owned_process(
                &mut child,
                group,
                deadline,
                context,
                &progress_rx,
            )?;
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

fn capture_stream(
    stream: &'static str,
    mut reader: impl Read,
    limits: super::OutputLimits,
    artifacts: super::ArtifactSink,
    progress: Sender<OperationProgress>,
) -> Result<OutputCapture, String> {
    let mut output = BoundedOutput::new(stream, limits, artifacts);
    let mut buffer = [0u8; 8192];
    let mut total = 0u64;
    loop {
        let count = reader.read(&mut buffer).map_err(|error| error.to_string())?;
        if count == 0 {
            break;
        }
        output
            .write_chunk(&buffer[..count])
            .map_err(|error| format!("failed capturing {stream}: {error}"))?;
        total = total.saturating_add(count as u64);
        let _ = progress.send(OperationProgress {
            stream,
            bytes: count as u64,
            total_bytes: total,
        });
    }
    output.finish().map_err(|error| error.to_string())
}

fn drain_progress(context: &OperationContext<'_>, progress: &Receiver<OperationProgress>) {
    while let Ok(event) = progress.try_recv() {
        context.progress(event);
    }
}

fn wait_owned_process(
    child: &mut Child,
    group: u32,
    deadline: Instant,
    context: &OperationContext<'_>,
    progress: &Receiver<OperationProgress>,
) -> Result<(ExitStatus, TerminationReason), String> {
    if context.cancellation().is_cancelled() {
        return terminate(child, group, TerminationReason::Cancelled);
    }
    loop {
        drain_progress(context, progress);
        if let Some(status) = child.try_wait().map_err(|error| error.to_string())? {
            cleanup_descendants(group);
            return Ok((status, TerminationReason::Completed));
        }
        if context.cancellation().is_cancelled() {
            return terminate(child, group, TerminationReason::Cancelled);
        }
        if Instant::now() >= deadline {
            return terminate(child, group, TerminationReason::TimedOut);
        }
        thread::sleep(POLL_INTERVAL);
    }
}

fn terminate(
    child: &mut Child,
    group: u32,
    reason: TerminationReason,
) -> Result<(ExitStatus, TerminationReason), String> {
    signal_group(group, Signal::Terminate);
    let grace_deadline = Instant::now() + TERMINATION_GRACE;
    loop {
        if let Some(status) = child.try_wait().map_err(|error| error.to_string())? {
            signal_group(group, Signal::Kill);
            return Ok((status, reason));
        }
        if Instant::now() >= grace_deadline {
            break;
        }
        thread::sleep(POLL_INTERVAL);
    }
    signal_group(group, Signal::Kill);
    let status = child.wait().map_err(|error| error.to_string())?;
    Ok((status, reason))
}

fn cleanup_descendants(group: u32) {
    signal_group(group, Signal::Terminate);
    thread::sleep(Duration::from_millis(20));
    signal_group(group, Signal::Kill);
}

#[derive(Clone, Copy)]
enum Signal {
    Terminate,
    Kill,
}

#[cfg(unix)]
fn configure_process_group(command: &mut Command) {
    use std::os::unix::process::CommandExt;
    unsafe {
        command.pre_exec(|| {
            if setpgid(0, 0) == 0 {
                Ok(())
            } else {
                Err(io::Error::last_os_error())
            }
        });
    }
}

#[cfg(not(unix))]
fn configure_process_group(_command: &mut Command) {}

#[cfg(unix)]
fn signal_group(group: u32, signal: Signal) {
    let number = match signal {
        Signal::Terminate => 15,
        Signal::Kill => 9,
    };
    unsafe {
        kill(-(group as i32), number);
    }
}

#[cfg(not(unix))]
fn signal_group(_group: u32, _signal: Signal) {}

#[cfg(unix)]
extern "C" {
    fn setpgid(pid: i32, pgid: i32) -> i32;
    fn kill(pid: i32, signal: i32) -> i32;
}
