//! Isolating and diffing a job workspace on Windows.
//!
//! Split out of `windows.rs` because the operator's rule keeps every source
//! file at three hundred lines or fewer, and the write guard answers an
//! oversized file by refusing every edit to it in both directions — so the
//! change below could not be made until the file was split. The FFI
//! declarations and constants deliberately stayed in the parent module, which
//! is why nothing here needed its visibility widened.
//!
//! `git worktree add --detach` is gone from `isolate`. The operator's rule is
//! "MA BYC NIEMOZLIWE UZYCIE WORKTREES. ZERO SUBAGENTOW NA OSOBNYCH
//! WORKTREES", and a job workspace is exactly the subagent case. What remains
//! is the recursive copy this file already carried, so an isolated workspace
//! registers nothing in the parent repository.
//!
//! The unix sibling, `platform/unix/workspace.rs`, was changed the same way
//! and keeps its clone paths; this platform has no clone path to keep.

use super::*;
use std::fs;
use std::io::Write;
use std::path::Path;
use std::process::{Command, Stdio};

impl WorkspacePlatform for NativePlatform {
    fn isolate(&self, parent: &Path, target: &Path) -> Result<&'static str, PlatformError> {
        copy_tree(parent, target)?;
        Ok("native-copy")
    }
    fn snapshot(
        &self,
        _parent: &Path,
        workspace: &Path,
        max: u64,
    ) -> Result<Vec<u8>, PlatformError> {
        if !workspace.join(".git").exists() {
            return Err(PlatformError::unsupported(
                "non-git workspace snapshot",
                UnsupportedReason::FilesystemCapability,
            ));
        }
        if !quiet(
            Command::new("git")
                .args(["add", "-N", "--all"])
                .current_dir(workspace),
        ) {
            return Err(PlatformError::Process("git add -N failed".into()));
        }
        let out = Command::new("git")
            .args([
                "diff",
                "--binary",
                "--no-ext-diff",
                "--src-prefix=a/",
                "--dst-prefix=b/",
            ])
            .current_dir(workspace)
            .output()?;
        if !out.status.success() {
            return Err(PlatformError::Process(
                String::from_utf8_lossy(&out.stderr).into_owned(),
            ));
        }
        if out.stdout.len() as u64 > max {
            return Err(PlatformError::Process(format!(
                "workspace snapshot exceeds {max} bytes"
            )));
        }
        Ok(out.stdout)
    }
    fn apply_snapshot(
        &self,
        parent: &Path,
        snapshot: &[u8],
        max: u64,
    ) -> Result<(), PlatformError> {
        if snapshot.is_empty() {
            return Ok(());
        }
        let mut child = Command::new("git")
            .args(["apply", "--3way", "--whitespace=nowarn", "-"])
            .current_dir(parent)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()?;
        child
            .stdin
            .take()
            .ok_or_else(|| PlatformError::Process("git apply stdin unavailable".into()))?
            .write_all(snapshot)?;
        let out = child.wait_with_output()?;
        if out.stderr.len() as u64 > max {
            return Err(PlatformError::Process(
                "git apply diagnostics exceeded limit".into(),
            ));
        }
        if out.status.success() {
            Ok(())
        } else {
            Err(PlatformError::Process(
                String::from_utf8_lossy(&out.stderr).into_owned(),
            ))
        }
    }
}

fn quiet(command: &mut Command) -> bool {
    command
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .is_ok_and(|s| s.success())
}

fn copy_tree(source: &Path, dest: &Path) -> Result<(), PlatformError> {
    fs::create_dir(dest)?;
    for item in fs::read_dir(source)? {
        let item = item?;
        let out = dest.join(item.file_name());
        if item.file_type()?.is_dir() {
            copy_tree(&item.path(), &out)?;
        } else {
            fs::copy(item.path(), out)?;
        }
    }
    Ok(())
}
