//! Isolating and diffing a job workspace on unix.
//!
//! Isolation uses an APFS clone on macOS, a reflink on Linux, or a recursive
//! copy when neither is available. It never registers a Git worktree in the
//! parent repository.

use super::super::*;
use super::UnixPlatform;
use std::fs;
use std::io::Write;
use std::process::{Command, Stdio};

impl WorkspacePlatform for UnixPlatform {
    fn isolate(&self, parent: &Path, target: &Path) -> Result<&'static str, PlatformError> {
        // A refused clone's cause is kept for the native copy's own refusal,
        // and the partial tree it left is removed first (see `cloned`): the
        // copy used to fail on that existing directory with neither cause said.
        let mut refused: Vec<String> = Vec::new();
        #[cfg(target_os = "macos")]
        match cloned(
            Command::new("cp")
                .arg("-cR")
                .arg(parent)
                .arg(target)
                .current_dir(parent),
            target,
        )? {
            None => return Ok("apfs-clone"),
            Some(cause) => refused.push(format!("cp -cR refused: {cause}")),
        }
        #[cfg(target_os = "linux")]
        match cloned(
            Command::new("cp")
                .args(["--reflink=auto", "-a"])
                .arg(parent)
                .arg(target)
                .current_dir(parent),
            target,
        )? {
            None => return Ok("reflink-copy"),
            Some(cause) => refused.push(format!("cp --reflink=auto refused: {cause}")),
        }
        copy_tree(parent, target).map_err(|error| match refused.is_empty() {
            true => error,
            false => PlatformError::Process(format!(
                "copying {} into {} failed: {error}; the clone before it: {}",
                parent.display(),
                target.display(),
                refused.join("; ")
            )),
        })?;
        Ok("native-copy")
    }
    fn snapshot(
        &self,
        parent: &Path,
        workspace: &Path,
        max: u64,
    ) -> Result<Vec<u8>, PlatformError> {
        if workspace.join(".git").exists() {
            checked(
                Command::new("git")
                    .args(["add", "-N", "--all"])
                    .current_dir(workspace),
                "git add -N --all while preparing the workspace snapshot",
            )?;
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

/// A workspace clone: `None` when it succeeded, else its exit and stderr, any partial tree at `target` removed.
fn cloned(command: &mut Command, target: &Path) -> Result<Option<String>, PlatformError> {
    let output = stado_wait::output(command.stdin(Stdio::null()))?;
    if output.status.success() {
        return Ok(None);
    }
    let cause = format!(
        "{} {}",
        output.status,
        String::from_utf8_lossy(&output.stderr).trim()
    );
    if target.exists() {
        fs::remove_dir_all(target)?;
    }
    Ok(Some(cause))
}

/// Run `command`, refusing its failure with what it was doing, its exit
/// status and what it printed on stderr.
fn checked(command: &mut Command, what: &str) -> Result<(), PlatformError> {
    let output = stado_wait::output(command.stdin(Stdio::null()))?;
    if output.status.success() {
        Ok(())
    } else {
        Err(PlatformError::Process(format!(
            "{what} failed with {}: {}",
            output.status,
            String::from_utf8_lossy(&output.stderr).trim()
        )))
    }
}

fn bounded(command: &mut Command, max: u64, allow_one: bool) -> Result<Vec<u8>, PlatformError> {
    let out = command.output()?;
    if out.stdout.len() as u64 > max {
        return Err(PlatformError::TooLarge {
            size: out.stdout.len() as u64,
            limit: max,
        });
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
