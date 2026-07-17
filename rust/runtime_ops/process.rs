use super::{
    platform::{native, ProcessSignal, ProcessTree},
    BoundedOutput, OperationContext, OperationProgress, OutputCapture,
};
use std::ffi::OsString;
use std::io::{self, Read, Write};
use std::path::PathBuf;
use std::process::{Child, Command, ExitStatus, Stdio};
use std::sync::mpsc::{self, Receiver, Sender};
use std::thread;
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
    pub preserve_descendants: bool,
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
            preserve_descendants: false,
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
        let mut builder =
            super::sandbox::command(&command.program, grant).map_err(|error| error.to_string())?;
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
            let deadline = context.effective_deadline(timeout);
            let (_progress_tx, progress_rx) = mpsc::channel();
            let (status, reason) = wait_owned_process(
                &mut child,
                process_tree.as_mut(),
                deadline,
                context,
                &progress_rx,
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
            let stderr_reader =
                scope.spawn(move || capture_stream("stderr", stderr, limits, artifacts, stderr_tx));
            drop(progress_tx);
            let stdin_writer = scope.spawn(move || -> io::Result<()> {
                if let (Some(mut pipe), Some(bytes)) = (stdin, command.stdin) {
                    pipe.write_all(&bytes)?;
                }
                Ok(())
            });

            let (status, reason) = wait_owned_process(
                &mut child,
                process_tree.as_mut(),
                deadline,
                context,
                &progress_rx,
                preserve_descendants,
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
        let count = reader
            .read(&mut buffer)
            .map_err(|error| error.to_string())?;
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
    process_tree: &mut dyn ProcessTree,
    deadline: Instant,
    context: &OperationContext<'_>,
    progress: &Receiver<OperationProgress>,
    preserve_descendants: bool,
) -> Result<(ExitStatus, TerminationReason), String> {
    if context.cancellation().is_cancelled() {
        return terminate(child, process_tree, TerminationReason::Cancelled);
    }
    loop {
        drain_progress(context, progress);
        if let Some(status) = child.try_wait().map_err(|error| error.to_string())? {
            if !preserve_descendants {
                cleanup_descendants(process_tree);
            }
            return Ok((status, TerminationReason::Completed));
        }
        if context.cancellation().is_cancelled() {
            return terminate(child, process_tree, TerminationReason::Cancelled);
        }
        if Instant::now() >= deadline {
            return terminate(child, process_tree, TerminationReason::TimedOut);
        }
        thread::sleep(POLL_INTERVAL);
    }
}

fn terminate(
    child: &mut Child,
    process_tree: &mut dyn ProcessTree,
    reason: TerminationReason,
) -> Result<(ExitStatus, TerminationReason), String> {
    process_tree
        .signal(ProcessSignal::Terminate)
        .map_err(|error| error.to_string())?;
    let grace_deadline = Instant::now() + TERMINATION_GRACE;
    loop {
        if let Some(status) = child.try_wait().map_err(|error| error.to_string())? {
            process_tree
                .signal(ProcessSignal::Kill)
                .map_err(|error| error.to_string())?;
            return Ok((status, reason));
        }
        if Instant::now() >= grace_deadline {
            break;
        }
        thread::sleep(POLL_INTERVAL);
    }
    process_tree
        .signal(ProcessSignal::Kill)
        .map_err(|error| error.to_string())?;
    let status = child.wait().map_err(|error| error.to_string())?;
    Ok((status, reason))
}

fn cleanup_descendants(process_tree: &mut dyn ProcessTree) {
    let _ = process_tree.signal(ProcessSignal::Terminate);
    thread::sleep(Duration::from_millis(20));
    let _ = process_tree.signal(ProcessSignal::Kill);
}

#[cfg(unix)]
fn configure_resource_limits(
    command: &mut Command,
    limits: super::ResourceLimits,
) -> Result<(), String> {
    use std::os::unix::process::CommandExt;
    if limits.cpu_seconds == 0
        || limits.address_space_bytes < 16 * 1024 * 1024
        || limits.open_files < 3
        || limits.processes == 0
        || limits.file_bytes == 0
    {
        return Err("invalid zero/unsafe process resource limit".into());
    }
    let inherited_fds = inherited_fds();
    unsafe {
        command.pre_exec(move || {
            mark_inherited_fds_close_on_exec(&inherited_fds);
            set_limit(RLIMIT_CPU, limits.cpu_seconds)?;
            #[cfg(target_os = "linux")]
            set_limit(RLIMIT_AS, limits.address_space_bytes)?;
            set_limit(RLIMIT_NOFILE, limits.open_files)?;
            #[cfg(target_os = "linux")]
            set_limit(RLIMIT_NPROC, limits.processes)?;
            set_limit(RLIMIT_FSIZE, limits.file_bytes)?;
            Ok(())
        });
    }
    Ok(())
}
#[cfg(not(unix))]
fn configure_resource_limits(
    _command: &mut Command,
    _limits: super::ResourceLimits,
) -> Result<(), String> {
    Err("native resource-limit backend unavailable".into())
}
#[cfg(target_os = "linux")]
fn inherited_fds() -> Vec<i32> {
    Vec::new()
}
#[cfg(target_os = "macos")]
fn inherited_fds() -> Vec<i32> {
    std::fs::read_dir("/dev/fd")
        .into_iter()
        .flatten()
        .filter_map(|entry| {
            entry
                .ok()?
                .file_name()
                .to_string_lossy()
                .parse::<i32>()
                .ok()
        })
        .filter(|fd| *fd > 2)
        .collect()
}
#[cfg(target_os = "linux")]
fn mark_inherited_fds_close_on_exec(_fds: &[i32]) {
    unsafe {
        syscall(436usize, 3u32, u32::MAX, 4u32);
    }
}
#[cfg(target_os = "macos")]
fn mark_inherited_fds_close_on_exec(fds: &[i32]) {
    for fd in fds {
        unsafe {
            fcntl(*fd, 2, 1);
        }
    }
}
#[cfg(unix)]
fn set_limit(resource: i32, value: u64) -> io::Result<()> {
    let limit = RLimit {
        current: value,
        maximum: value,
    };
    if unsafe { setrlimit(resource, &limit) } == 0 {
        Ok(())
    } else {
        Err(io::Error::last_os_error())
    }
}
#[cfg(unix)]
#[repr(C)]
struct RLimit {
    current: u64,
    maximum: u64,
}
#[cfg(unix)]
extern "C" {
    fn setrlimit(resource: i32, limit: *const RLimit) -> i32;
}
#[cfg(target_os = "linux")]
extern "C" {
    fn syscall(number: usize, ...) -> isize;
}
#[cfg(target_os = "macos")]
extern "C" {
    fn fcntl(fd: i32, command: i32, ...) -> i32;
}
#[cfg(unix)]
const RLIMIT_CPU: i32 = 0;
#[cfg(unix)]
const RLIMIT_FSIZE: i32 = 1;
#[cfg(target_os = "linux")]
const RLIMIT_NPROC: i32 = 6;
#[cfg(target_os = "linux")]
const RLIMIT_NOFILE: i32 = 7;
#[cfg(target_os = "linux")]
const RLIMIT_AS: i32 = 9;
#[cfg(target_os = "macos")]
const RLIMIT_NOFILE: i32 = 8;
