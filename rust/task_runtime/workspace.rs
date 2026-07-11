use super::types::TaskError;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::io::{Read, Write};
use std::thread;

struct BoundedCommandOutput { success: bool, code: Option<i32>, stdout: Vec<u8>, stderr: Vec<u8>, truncated: bool }

fn read_bounded(mut input: impl Read, max: usize) -> Result<(Vec<u8>, bool), TaskError> {
    let mut output = Vec::with_capacity(max.min(64 * 1024));
    let mut buffer = [0u8; 8192];
    let mut truncated = false;
    loop {
        let count = input.read(&mut buffer)?;
        if count == 0 { break; }
        let keep = max.saturating_sub(output.len()).min(count);
        output.extend_from_slice(&buffer[..keep]);
        truncated |= keep < count;
    }
    Ok((output, truncated))
}

fn bounded_output(command: &mut Command, max_bytes: u64, stdin_data: Option<&[u8]>) -> Result<BoundedCommandOutput, TaskError> {
    let mut child = command.stdin(if stdin_data.is_some() { Stdio::piped() } else { Stdio::null() }).stdout(Stdio::piped()).stderr(Stdio::piped()).spawn()?;
    let stdout = child.stdout.take().ok_or_else(|| TaskError::Process("capture stdout unavailable".into()))?;
    let stderr = child.stderr.take().ok_or_else(|| TaskError::Process("capture stderr unavailable".into()))?;
    let stdin = child.stdin.take();
    let (status, stdout, stderr, writer) = thread::scope(|scope| {
        let out = scope.spawn(|| read_bounded(stdout, max_bytes as usize));
        let err = scope.spawn(|| read_bounded(stderr, 64 * 1024));
        let writer = scope.spawn(move || -> Result<(), TaskError> {
            if let (Some(mut stdin), Some(data)) = (stdin, stdin_data) { stdin.write_all(data)?; }
            Ok(())
        });
        let status = child.wait();
        (status, out.join(), err.join(), writer.join())
    });
    let status = status?;
    writer.map_err(|_| TaskError::Process("capture stdin writer panicked".into()))??;
    let (stdout, truncated) = stdout.map_err(|_| TaskError::Process("capture stdout reader panicked".into()))??;
    let (stderr, stderr_truncated) = stderr.map_err(|_| TaskError::Process("capture stderr reader panicked".into()))??;
    Ok(BoundedCommandOutput { success: status.success(), code: status.code(), stdout, stderr, truncated: truncated || stderr_truncated })
}

#[derive(Clone, Debug)]
pub struct IsolatedWorkspace { pub path: PathBuf, pub strategy: String, pub(crate) parent: PathBuf }

fn run(program: &str, args: &[&str], cwd: &Path) -> bool { Command::new(program).args(args).current_dir(cwd).stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null()).status().map(|s| s.success()).unwrap_or(false) }

pub fn isolate(parent: &Path, root: &Path, id: &str) -> Result<IsolatedWorkspace, TaskError> {
    fs::create_dir_all(root)?;
    let target = root.join(id);
    if target.exists() { return Err(TaskError::Conflict(format!("workspace already exists: {}", target.display()))); }
    let parent_text = parent.to_string_lossy(); let target_text = target.to_string_lossy();
    if cfg!(target_os = "macos") && run("cp", &["-cR", parent_text.as_ref(), target_text.as_ref()], parent) {
        return Ok(IsolatedWorkspace { path: target, strategy: "apfs-clone".into(), parent: parent.into() });
    }
    if parent.join(".git").exists() && run("git", &["worktree", "add", "--detach", target_text.as_ref(), "HEAD"], parent) {
        return Ok(IsolatedWorkspace { path: target, strategy: "git-worktree".into(), parent: parent.into() });
    }
    if run("cp", &["-R", parent_text.as_ref(), target_text.as_ref()], parent) {
        return Ok(IsolatedWorkspace { path: target, strategy: "copy".into(), parent: parent.into() });
    }
    Err(TaskError::Io("no workspace isolation strategy succeeded".into()))
}

impl IsolatedWorkspace {
    pub fn capture(&self, destination: &Path, max_bytes: u64) -> Result<(), TaskError> {
        if let Some(parent) = destination.parent() { fs::create_dir_all(parent)?; }
        let output = if self.path.join(".git").exists() {
            let intent = Command::new("git").args(["add", "-N", "--all"]).current_dir(&self.path).stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null()).status()?;
            if !intent.success() { return Err(TaskError::Process("git add -N failed while preparing workspace capture".into())); }
            bounded_output(Command::new("git").args(["diff", "--binary", "--no-ext-diff", "--src-prefix=a/", "--dst-prefix=b/"]).current_dir(&self.path), max_bytes, None)?
        } else {
            bounded_output(Command::new("diff").args(["-ruN", self.parent.to_string_lossy().as_ref(), self.path.to_string_lossy().as_ref()]), max_bytes, None)?
        };
        if output.truncated { return Err(TaskError::Capacity { running: output.stdout.len(), limit: max_bytes as usize }); }
        if !output.success && output.code != Some(1) { return Err(TaskError::Process(String::from_utf8_lossy(&output.stderr).into_owned())); }
        fs::write(destination, output.stdout)?;
        Ok(())
    }
    pub fn merge(&self, capture: &Path) -> Result<(), TaskError> {
        if !capture.exists() { return Err(TaskError::NotFound(format!("capture not found: {}", capture.display()))); }
        let data = fs::read(capture)?;
        if data.is_empty() { return Ok(()); }
        let output = bounded_output(Command::new("git").args(["apply", "--3way", "--whitespace=nowarn", "-"]).current_dir(&self.parent), 64 * 1024, Some(&data))?;
        if output.truncated { return Err(TaskError::Conflict("git apply diagnostics exceeded capture limit".into())); }
        if !output.success { return Err(TaskError::Conflict(String::from_utf8_lossy(&output.stderr).into_owned())); }
        Ok(())
    }
}
