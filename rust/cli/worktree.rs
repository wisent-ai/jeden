//! `jeden worktree` — enumerate and remove git worktrees found under this
//! project. It is a cleanup tool only: this product cannot create a worktree.
//!
//! What it can find, and why:
//! - Job workspaces are isolated by copying, never by `git worktree add`.
//!   `platform/unix/workspace.rs` returns `apfs-clone`, `reflink-copy` or
//!   `native-copy`, and `platform/windows/workspace.rs` returns
//!   `native-copy`. Those workspaces are not worktrees and are counted
//!   separately from the rows below.
//! - So every worktree this command lists came from somewhere else: an older
//!   version of this runtime that did register them, another tool, or a
//!   person. That is precisely why the command still exists.
//! - Job records (`<store>/jobs/*.json`) name the workspaces this project
//!   manages; orphaned directories under managed workspace roots are found by
//!   scanning the roots, and anything else the repository itself knows about
//!   comes from its own `git worktree list`.
//! - This module is read-only with respect to scheduler stores: it never
//!   opens a `TaskScheduler` (which would create store directories).

mod clear;
mod discovery;

use clear::render_clear;
use discovery::{collect_managed, repo_worktrees};

use crate::task_runtime::{default_store, workspace_root_for, JobRecord, JobStatus};
use crate::{session_root, Args};
use serde::Serialize;
use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{SystemTime, UNIX_EPOCH};

#[cfg(unix)]
unsafe extern "C" {
    fn kill(pid: i32, signal: i32) -> i32;
}

#[cfg(unix)]
fn process_alive(pid: u32) -> bool {
    unsafe { kill(pid as i32, 0) == 0 }
}

#[cfg(not(unix))]
fn process_alive(_pid: u32) -> bool {
    false
}

#[derive(Clone, Debug)]
struct ManagedWorktree {
    path: PathBuf,
    branch: String,
    created_ms: u64,
    job_id: Option<String>,
    status: Option<JobStatus>,
    pid: Option<u32>,
    /// Repository that registered the worktree (job cwd, or parsed from the
    /// worktree gitfile for orphaned workspaces).
    parent_repo: Option<PathBuf>,
    /// Administrative dir `<repo>/.git/worktrees/<name>`, when known.
    admin_dir: Option<PathBuf>,
}

impl ManagedWorktree {
    fn running(&self) -> bool {
        if self.pid.map(process_alive).unwrap_or(false) {
            return true;
        }
        matches!(
            self.status,
            Some(JobStatus::Queued) | Some(JobStatus::Running) | Some(JobStatus::Waiting)
        )
    }

    fn stale(&self) -> bool {
        !self.running()
    }
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct WorktreeRow {
    path: String,
    branch: String,
    age: String,
    age_ms: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    job_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    status: Option<String>,
    stale: bool,
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}

fn format_age(age_ms: u64) -> String {
    let seconds = age_ms / 1_000;
    let minutes = seconds / 60;
    let hours = minutes / 60;
    let days = hours / 24;
    if days > 0 {
        format!("{days}d {}h", hours % 24)
    } else if hours > 0 {
        format!("{hours}h {}m", minutes % 60)
    } else if minutes > 0 {
        format!("{minutes}m")
    } else {
        format!("{seconds}s")
    }
}

fn modified_ms(path: &Path) -> u64 {
    fs::metadata(path)
        .and_then(|meta| meta.modified())
        .ok()
        .and_then(|time| time.duration_since(UNIX_EPOCH).ok())
        .map(|duration| duration.as_millis() as u64)
        .unwrap_or(0)
}

fn status_name(status: &JobStatus) -> &'static str {
    match status {
        JobStatus::Queued => "queued",
        JobStatus::Running => "running",
        JobStatus::Waiting => "waiting",
        JobStatus::Succeeded => "succeeded",
        JobStatus::Failed => "failed",
        JobStatus::Cancelled => "cancelled",
        JobStatus::Interrupted => "interrupted",
    }
}

fn git(path: &Path, args: &[&str]) -> Option<String> {
    let output = Command::new("git")
        .args(args)
        .current_dir(path)
        .output()
        .ok()?;
    if output.status.success() {
        Some(String::from_utf8_lossy(&output.stdout).trim().to_string())
    } else {
        None
    }
}

/// A path is a git worktree checkout when its `.git` is the gitfile that
/// points back to `<repo>/.git/worktrees/<name>` (a directory `.git` means a
/// full clone/copy, not a worktree).
fn is_worktree_checkout(path: &Path) -> bool {
    path.join(".git").is_file()
}

fn branch_of(path: &Path) -> String {
    match git(path, &["branch", "--show-current"]) {
        Some(branch) if !branch.is_empty() => branch,
        _ => match git(path, &["rev-parse", "--short", "HEAD"]) {
            Some(sha) if !sha.is_empty() => format!("detached@{sha}"),
            _ => "-".to_string(),
        },
    }
}

/// Parse a worktree gitfile (`gitdir: <repo>/.git/worktrees/<name>`) into the
/// parent repository root and the administrative directory.
fn parse_gitfile(path: &Path) -> Option<(PathBuf, PathBuf)> {
    let content = fs::read_to_string(path.join(".git")).ok()?;
    let gitdir = content.trim().strip_prefix("gitdir:")?.trim();
    let marker = format!(
        "{}.git{}worktrees{}",
        std::path::MAIN_SEPARATOR,
        std::path::MAIN_SEPARATOR,
        std::path::MAIN_SEPARATOR
    );
    let index = gitdir.find(&marker)?;
    Some((PathBuf::from(&gitdir[..index]), PathBuf::from(gitdir)))
}


fn to_row(worktree: &ManagedWorktree, now: u64) -> WorktreeRow {
    WorktreeRow {
        path: worktree.path.display().to_string(),
        branch: worktree.branch.clone(),
        age: format_age(now.saturating_sub(worktree.created_ms)),
        age_ms: now.saturating_sub(worktree.created_ms),
        job_id: worktree.job_id.clone(),
        status: worktree
            .status
            .as_ref()
            .map(status_name)
            .map(str::to_string),
        stale: worktree.stale(),
    }
}


fn render_list(args: &Args) -> String {
    let cwd = &args.cwd;
    let now = now_ms();
    let (managed, clone_workspaces) = collect_managed(cwd);
    if managed.is_empty() {
        let note = "no git worktrees found under this project's workspace roots (job workspaces are isolated by copying, so they are not worktrees and are not listed)";
        match repo_worktrees(cwd) {
            Some(repo) => {
                let rows: Vec<WorktreeRow> = repo.iter().map(|w| to_row(w, now)).collect();
                if args.json {
                    return serde_json::to_string_pretty(&serde_json::json!({
                        "managed": false,
                        "note": note,
                        "source": "git worktree list",
                        "worktrees": rows,
                    }))
                    .unwrap_or_default()
                        + "\n";
                }
                let mut out = format!(
                    "{note}\nrepository worktrees from `git worktree list` ({}):\n",
                    rows.len()
                );
                for row in &rows {
                    out.push_str(&format!("  {} · {} · {}\n", row.path, row.branch, row.age));
                }
                out
            }
            None => {
                if args.json {
                    return serde_json::to_string_pretty(&serde_json::json!({
                        "managed": false,
                        "note": format!("{note}; {} is not inside a git repository", cwd.display()),
                        "worktrees": [],
                    }))
                    .unwrap_or_default()
                        + "\n";
                }
                format!("{note}; {} is not inside a git repository\n", cwd.display())
            }
        }
    } else {
        let rows: Vec<WorktreeRow> = managed.iter().map(|w| to_row(w, now)).collect();
        if args.json {
            return serde_json::to_string_pretty(&serde_json::json!({
                "managed": true,
                "cloneIsolatedWorkspaces": clone_workspaces,
                "worktrees": rows,
            }))
            .unwrap_or_default()
                + "\n";
        }
        let mut out = format!(
            "git worktrees under {} ({}):\n",
            cwd.display(),
            rows.len()
        );
        for row in &rows {
            out.push_str(&format!("  {} · {} · {}\n", row.path, row.branch, row.age));
        }
        if clone_workspaces > 0 {
            out.push_str(&format!(
                "({clone_workspaces} clone-isolated workspace(s) are not git worktrees and are not listed)\n"
            ));
        }
        out
    }
}


pub(crate) fn worktree_command(args: &Args) -> Result<String, String> {
    let mut action: Option<&str> = None;
    let mut dry_run = false;
    for token in &args.positionals {
        match token.as_str() {
            "--dry-run" => dry_run = true,
            "--json" => {} // handled globally by parse_args
            "list" | "clear" if action.is_none() => {
                action = Some(match token.as_str() {
                    "clear" => "clear",
                    _ => "list",
                })
            }
            other => {
                return Err(format!(
                    "unexpected argument '{other}': usage: jeden worktree [list|clear] [--dry-run] [--json]"
                ))
            }
        }
    }
    match action.unwrap_or("list") {
        "clear" => Ok(render_clear(args, dry_run)),
        _ => Ok(render_list(args)),
    }
}
