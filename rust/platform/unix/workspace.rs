//! Isolating and diffing a job workspace on unix.
//!
//! Split out of `unix.rs` because the operator's rule keeps every source file
//! at three hundred lines or fewer, and the write guard answers an oversized
//! file by refusing every edit to it in both directions — so the change below
//! could not be made until the file was split.
//!
//! `git worktree add` is gone from this path. The operator's instruction was
//! "MA BYC NIEMOZLIWE UZYCIE WORKTREES. ZERO SUBAGENTOW NA OSOBNYCH
//! WORKTREES", and a job workspace is exactly the subagent case. Isolation is
//! by copy: an APFS clone on macOS, a reflink on Linux, and a plain recursive
//! copy when neither is available. None of them registers a worktree in the
//! parent repository, so `jeden worktree` has nothing of ours to clean up.
//!
//! The removed branch was already unreachable on this machine, because the
//! APFS clone above it always succeeds on an APFS volume. Removing it is
//! still the point: a capability that still exists is one that returns the
//! moment the filesystem underneath changes.

use super::super::*;
use super::UnixPlatform;
use std::fs;
use std::io::Write;
use std::process::{Command, Stdio};

impl WorkspacePlatform for UnixPlatform {
    fn isolate(&self, parent: &Path, target: &Path) -> Result<&'static str, PlatformError> {
        #[cfg(target_os = "macos")]
        if quiet(
            Command::new("cp")
                .arg("-cR")
                .arg(parent)
                .arg(target)
                .current_dir(parent),
        ) {
            return Ok("apfs-clone");
        }
        #[cfg(target_os = "linux")]
        if quiet(
            Command::new("cp")
                .args(["--reflink=auto", "-a"])
                .arg(parent)
                .arg(target)
                .current_dir(parent),
        ) {
            return Ok("reflink-copy");
        }
        copy_tree(parent, target)?;
        Ok("native-copy")
    }
    fn snapshot(
        &self,
        parent: &Path,
        workspace: &Path,
        max: u64,
    ) -> Result<Vec<u8>, PlatformError> {
        if workspace.join(".git").exists() {
            if !quiet(
                Command::new("git")
                    .args(["add", "-N", "--all"])
                    .current_dir(workspace),
            ) {
                return Err(PlatformError::Process(
                    "git add -N failed while preparing workspace snapshot".into(),
                ));
            }
            bounded(
                Command::new("git")
                    .args([
                        "diff",
                        "--binary",
                        "--no-ext-diff",
                        "--src-prefix=a/",
                        "--dst-prefix=b/",
                    ])
                    .current_dir(workspace),
                max,
                true,
            )
        } else {
            bounded(
                Command::new("diff")
                    .args(["-ruN"])
                    .arg(parent)
                    .arg(workspace),
                max,
                true,
            )
        }
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
        let (ok, _, stderr) = bounded_with_stdin(
            Command::new("git")
                .args(["apply", "--3way", "--whitespace=nowarn", "-"])
                .current_dir(parent),
            max,
            snapshot,
        )?;
        if ok {
            Ok(())
        } else {
            Err(PlatformError::Process(
                String::from_utf8_lossy(&stderr).into_owned(),
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

fn bounded(command: &mut Command, max: u64, allow_one: bool) -> Result<Vec<u8>, PlatformError> {
    let out = command.output()?;
    if out.stdout.len() as u64 > max {
        return Err(PlatformError::Process(format!(
            "workspace snapshot exceeds {max} bytes"
        )));
    }
    if !out.status.success() && !(allow_one && out.status.code() == Some(1)) {
        return Err(PlatformError::Process(
            String::from_utf8_lossy(&out.stderr).into_owned(),
        ));
    }
    Ok(out.stdout)
}

fn bounded_with_stdin(
    command: &mut Command,
    max: u64,
    input: &[u8],
) -> Result<(bool, Vec<u8>, Vec<u8>), PlatformError> {
    let mut child = command
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()?;
    child
        .stdin
        .take()
        .ok_or_else(|| PlatformError::Process("snapshot stdin unavailable".into()))?
        .write_all(input)?;
    let out = child.wait_with_output()?;
    if out.stdout.len() as u64 > max || out.stderr.len() as u64 > max {
        return Err(PlatformError::Process(format!(
            "workspace diagnostics exceed {max} bytes"
        )));
    }
    Ok((out.status.success(), out.stdout, out.stderr))
}

fn copy_tree(source: &Path, dest: &Path) -> Result<(), PlatformError> {
    fs::create_dir(dest)?;
    for item in fs::read_dir(source)? {
        let item = item?;
        let ty = item.file_type()?;
        let out = dest.join(item.file_name());
        if ty.is_dir() {
            copy_tree(&item.path(), &out)?;
        } else if ty.is_symlink() {
            let target = fs::read_link(item.path())?;
            std::os::unix::fs::symlink(target, out)?;
        } else {
            fs::copy(item.path(), out)?;
        }
    }
    Ok(())
}
