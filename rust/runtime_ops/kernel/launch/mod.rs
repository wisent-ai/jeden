use super::super::platform::{native, PipeReader};
use super::bootstrap::{JAVASCRIPT_BOOTSTRAP, PYTHON_BOOTSTRAP};
use super::control::{KernelControl, SharedControl};
use super::{ExecutionGrant, KernelLanguage};
use std::ffi::OsStr;
use std::path::Path;
use std::process::{ChildStdin, Command, Stdio};
use std::sync::{Arc, Mutex};

pub(super) struct Spawned {
    pub(super) control: SharedControl,
    pub(super) stdin: ChildStdin,
    pub(super) stdout: Box<dyn PipeReader>,
    pub(super) stderr: Box<dyn PipeReader>,
}

pub(super) fn spawn(language: KernelLanguage, cwd: &Path, grant: &ExecutionGrant) -> Result<Spawned, String> {
    let program = match language {
        KernelLanguage::Python => OsStr::new("python3"),
        KernelLanguage::JavaScript => OsStr::new("node"),
    };
    let mut command = Command::new(program);
    match language {
        KernelLanguage::Python => { command.args(["-u", "-c", PYTHON_BOOTSTRAP]); }
        KernelLanguage::JavaScript => { command.args(["-e", JAVASCRIPT_BOOTSTRAP]); }
    }
    command.current_dir(cwd).stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::piped());
    command.env_clear();
    for key in &grant.process.environment {
        if let Some(value) = std::env::var_os(key) { command.env(key, value); }
    }
    native().configure_command(&mut command).map_err(|cause| cause.to_string())?;
    let mut child = command.spawn().map_err(|cause| format!("failed launching {} kernel: {cause}", language.label()))?;
    let tree = match native().attach_process_tree(&child) {
        Ok(tree) => tree,
        Err(cause) => {
            let original = format!("attach {} kernel process {} to its process tree: {cause}", language.label(), child.id());
            // No input has been sent. Until attachment succeeds, this child is
            // the only process handle the launch actually owns.
            match child.kill() {
                Ok(()) => {
                    let process = child.id();
                    let waited = stado_wait::blocking(stado_wait::Kind::Process,
                        format!("unattached kernel process {process} to end"), "this host", || child.wait());
                    if let Err(cleanup) = waited { return Err(format!("{original}; await stopped child: {cleanup}")); }
                }
                Err(cleanup) => {
                    let observed = match child.try_wait() {
                        Ok(Some(status)) => format!("child already exited ({status})"),
                        Ok(None) => "child has not exited".to_owned(),
                        Err(status) => format!("child status could not be read: {status}"),
                    };
                    return Err(format!("{original}; stop unattached child: {cleanup}; {observed}"));
                }
            }
            return Err(original);
        }
    };
    // From here onward every pipe setup error drops native ownership, which
    // stops and reaps the interpreter instead of abandoning std::process::Child.
    let mut control = KernelControl::new(child, tree, language);
    let stdin = control.child.stdin.take().ok_or_else(|| format!("{} kernel process {} stdin unavailable", language.label(), control.pid()))?;
    let stdout = native().pipe_reader(Box::new(control.child.stdout.take()
        .ok_or_else(|| format!("{} kernel process {} stdout unavailable", language.label(), control.pid()))?))
        .map_err(|cause| format!("read {} kernel process {} stdout: {cause}", language.label(), control.pid()))?;
    let stderr = native().pipe_reader(Box::new(control.child.stderr.take()
        .ok_or_else(|| format!("{} kernel process {} stderr unavailable", language.label(), control.pid()))?))
        .map_err(|cause| format!("read {} kernel process {} stderr: {cause}", language.label(), control.pid()))?;
    control.wake = Some(stdout.waker());
    super::super::wake_on_cancellation(stdout.waker());
    Ok(Spawned { control: Arc::new(Mutex::new(control)), stdin, stdout, stderr })
}
