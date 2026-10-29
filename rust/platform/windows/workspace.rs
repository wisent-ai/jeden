//! Isolating and diffing a job workspace on Windows.
//!
//! Isolation is a recursive copy, so an isolated workspace registers nothing
//! in the parent repository (no Git worktree). Snapshots are taken and applied
//! through the shared captured runs, which name every wait and feed git
//! apply's stdin from its own thread.

use super::*;
use crate::tool_runtime::runtime_ops::captured::{bounded, bounded_with_stdin, checked};
use std::fs;
use std::path::Path;
use std::process::Command;

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
        if !workspace.join(".git").try_exists()? {
            return Err(PlatformError::unsupported(
                "non-git workspace snapshot",
                UnsupportedReason::FilesystemCapability,
            ));
        }
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
