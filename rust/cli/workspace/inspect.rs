//! Reading an existing directory and reporting what it is: whether it sits in
//! a Git repository, whether it carries a project configuration, and how many
//! sealed sessions already name it.
//!
//! Split out of `cli/workspace.rs` on the seam between establishing facts
//! about a directory and running the `workspace` command over them, because
//! the operator's rule keeps every source file at three hundred lines or
//! fewer and the write guard refuses every edit to a file above that.
//!
//! `source` used to read `git-worktree` whenever a repository root was found.
//! That was never measured: the check below tests for `.git` up the ancestors
//! and learns only that the directory belongs to a repository, never how the
//! checkout was made. It now says that instead. Nothing in this product
//! creates a worktree any more either — `platform/unix/workspace.rs` isolates
//! by copy and returns `apfs-clone`, `reflink-copy` or `native-copy`.

use serde_json::Value;
use std::fs;
use std::path::{Path, PathBuf};

use super::{WorkspaceReport, WorkspaceSessions};
use crate::session_root;

fn resolve_input(path: &Path, base: &Path) -> PathBuf {
    if path.is_absolute() {
        path.to_path_buf()
    } else {
        base.join(path)
    }
}

/// The nearest ancestor holding `.git`. A `.git` that cannot be examined is
/// refused naming it, never read as absent and the workspace reported as a
/// plain directory.
fn repository_root(workspace: &Path) -> Result<Option<PathBuf>, String> {
    for candidate in workspace.ancestors() {
        let marker = candidate.join(".git");
        match marker.try_exists() {
            Ok(true) => return Ok(Some(candidate.to_path_buf())),
            Ok(false) => {}
            Err(error) => return Err(format!("{} cannot be examined: {error}", marker.display())),
        }
    }
    Ok(None)
}

fn validate_project_config(workspace: &Path) -> Result<String, String> {
    let path = workspace.join(".jeden/config.json");
    let bytes = match fs::read(&path) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Ok("no project config; user defaults apply".into())
        }
        Err(error) => {
            return Err(format!(
                "cannot read existing Jeden configuration {}: {error}",
                path.display()
            ))
        }
    };
    let value: Value = serde_json::from_slice(&bytes).map_err(|error| {
        format!(
            "invalid existing Jeden configuration {}: {error}",
            path.display()
        )
    })?;
    if !value.is_object() {
        return Err(format!(
            "invalid existing Jeden configuration {}: root must be an object",
            path.display()
        ));
    }
    Ok(format!("accepted existing {}", path.display()))
}

/// Every sealed session whose recorded `cwd` lies in `workspace`, and every
/// session directory that could not be read, named with its cause.
fn session_counts(workspace: &Path) -> Result<WorkspaceSessions, String> {
    let mut accepted = Vec::new();
    let mut rejections = Vec::new();
    let root = session_root();
    let entries = match fs::read_dir(&root) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Ok(WorkspaceSessions::from_outcomes(&accepted, rejections))
        }
        Err(error) => {
            return Err(format!(
                "cannot read canonical session root {}: {error}",
                root.display()
            ))
        }
    };
    for entry in entries {
        let entry = match entry {
            Ok(entry) => entry,
            Err(error) => {
                rejections.push(format!("an entry of {}: {error}", root.display()));
                continue;
            }
        };
        let state_path = entry.path().join("state.json");
        match fs::metadata(&state_path) {
            Ok(metadata) if metadata.is_file() => {}
            Ok(_not_file) => continue,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
            Err(error) => {
                rejections.push(format!("{}: {error}", state_path.display()));
                continue;
            }
        }
        let state = match fs::read(&state_path)
            .map_err(|error| error.to_string())
            .and_then(|bytes| {
                serde_json::from_slice::<Value>(&bytes).map_err(|error| error.to_string())
            }) {
            Ok(value) => value,
            Err(cause) => {
                rejections.push(format!("{}: {cause}", state_path.display()));
                continue;
            }
        };
        let Some(cwd) = state.get("cwd").and_then(Value::as_str) else {
            rejections.push(format!("{} records no cwd", state_path.display()));
            continue;
        };
        let cwd = match Path::new(cwd).canonicalize() {
            Ok(cwd) => cwd,
            Err(error) => {
                rejections.push(format!(
                    "{} records cwd {cwd}, which cannot be resolved: {error}",
                    state_path.display()
                ));
                continue;
            }
        };
        if cwd.starts_with(workspace) {
            accepted.push(entry.path());
        }
    }
    Ok(WorkspaceSessions::from_outcomes(&accepted, rejections))
}

pub(crate) fn inspect(path: &Path, base: &Path, status: &str) -> Result<WorkspaceReport, String> {
    let requested = resolve_input(path, base);
    if requested
        .components()
        .any(|component| matches!(component, std::path::Component::ParentDir))
    {
        return Err(format!(
            "workspace path must not contain '..': {}",
            requested.display()
        ));
    }
    let workspace = requested.canonicalize().map_err(|error| {
        format!(
            "workspace {} is not an existing readable directory: {error}",
            requested.display()
        )
    })?;
    if !workspace.is_dir() {
        return Err(format!(
            "workspace {} is not an existing directory",
            workspace.display()
        ));
    }
    fs::read_dir(&workspace).map_err(|error| {
        format!(
            "workspace {} is not a readable directory: {error}",
            workspace.display()
        )
    })?;
    let configuration = validate_project_config(&workspace)?;
    let repository_root = repository_root(&workspace)?;
    // What the check above establishes is that an ancestor holds `.git`. It
    // does not establish how that checkout came to exist, so the report names
    // the fact and leaves the mechanism unclaimed.
    let source = if repository_root.is_some() {
        "git-repository"
    } else {
        "directory"
    };
    let sessions = session_counts(&workspace)?;
    Ok(WorkspaceReport::observed(
        status,
        workspace,
        source,
        repository_root,
        configuration,
        sessions,
    ))
}
