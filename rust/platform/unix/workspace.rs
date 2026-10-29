//! Isolating and diffing a job workspace on unix.
//!
//! Isolation uses an APFS clone on macOS, a reflink on Linux, or a recursive
//! copy when neither is available. It never registers a Git worktree in the
//! parent repository.

use super::super::*;
use super::UnixPlatform;
use crate::tool_runtime::runtime_ops::captured::{bounded, bounded_with_stdin, checked};
use std::fs;
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
        copy_tree(parent, target).map_err(|error| {
            if refused.is_empty() {
                error
            } else {
                PlatformError::Process(format!(
                    "copying {} into {} failed: {error}; the clone before it: {}",
                    parent.display(),
                    target.display(),
                    refused.join("; ")
                ))
            }
        })?;
        Ok("native-copy")
    }
    fn snapshot(
        &self,
        parent: &Path,
        workspace: &Path,
        max: u64,
    ) -> Result<Vec<u8>, PlatformError> {
        // A `.git` that cannot be examined is refused, never snapshotted as a
        // plain directory.
        if workspace.join(".git").try_exists()? {
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
                // git diff without --exit-code succeeds whatever it finds.
                false,
            )
        } else {
            bounded(
                Command::new("diff")
                    .args(["-ruN"])
                    .arg(parent)
                    .arg(workspace),
                max,
                // diff(1) answers found differences with a failing status.
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
        let (ok, _stdout, stderr) = bounded_with_stdin(
            Command::new("git")
                .args(["apply", "--3way", "--whitespace=nowarn", "-"])
                .current_dir(parent),
            max,
            snapshot,
        )?;
        if ok {
            Ok(())
        } else {
            Err(PlatformError::Process(format!(
                "git apply --3way of the workspace snapshot onto {} failed: {}",
                parent.display(),
                String::from_utf8_lossy(&stderr).trim()
            )))
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
    // Only a partial tree that is not there needs no removal.
    match fs::remove_dir_all(target) {
        Ok(()) => {}
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => return Err(error.into()),
    }
    Ok(Some(cause))
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
