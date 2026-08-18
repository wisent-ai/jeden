//! `jeden worktree` — enumerate and clean up the git worktrees that jeden's
//! task runtime (`task`/`job` tools, `/tan`, delegation) creates when it
//! isolates job workspaces with the `git-worktree` strategy.
//!
//! Truthfulness notes:
//! - The runtime prefers clone-based isolation where available (`apfs-clone`
//!   on macOS, `reflink-copy` on Linux) and only falls back to
//!   `git worktree add --detach` (see `platform::unix::isolate`). Workspaces
//!   isolated by cloning are *not* git worktrees and are never listed here.
//! - Job records (`<store>/jobs/*.json`) are the source of truth for which
//!   workspaces jeden manages; orphaned workspace directories under managed
//!   workspace roots are picked up by scanning the roots directly.
//! - This module is read-only with respect to scheduler stores: it never
//!   opens a `TaskScheduler` (which would create store directories).

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

fn read_jobs(store: &Path) -> Vec<JobRecord> {
    let mut jobs = Vec::new();
    let Ok(entries) = fs::read_dir(store.join("jobs")) else {
        return jobs;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().and_then(|ext| ext.to_str()) != Some("json") {
            continue;
        }
        if let Ok(job) = serde_json::from_slice::<JobRecord>(&fs::read(&path).unwrap_or_default()) {
            jobs.push(job);
        }
    }
    jobs
}

/// Scheduler stores that may hold job records relevant to `cwd`.
fn candidate_stores(cwd: &Path) -> Vec<PathBuf> {
    let mut stores = Vec::new();
    let mut seen = BTreeSet::new();
    let push = |store: PathBuf, seen: &mut BTreeSet<PathBuf>, stores: &mut Vec<PathBuf>| {
        if store.is_dir() && seen.insert(store.clone()) {
            stores.push(store);
        }
    };
    if let Some(store) = std::env::var_os("JEDEN_TASK_STORE") {
        push(PathBuf::from(store), &mut seen, &mut stores);
    }
    // Default store for the `task`/`job` tools and delegation.
    push(default_store(cwd, None), &mut seen, &mut stores);
    // Store probed by `jeden doctor`.
    push(cwd.join(".jeden/tasks"), &mut seen, &mut stores);
    // Per-session stores used by `/tan` background jobs.
    if let Ok(entries) = fs::read_dir(session_root()) {
        for entry in entries.flatten() {
            let store = entry.path().join("task-runtime");
            push(store, &mut seen, &mut stores);
        }
    }
    stores
}

fn same_dir(a: &Path, b: &Path) -> bool {
    let canon = |p: &Path| fs::canonicalize(p).unwrap_or_else(|_| p.to_path_buf());
    canon(a) == canon(b)
}

/// Collect every git worktree the task runtime manages for this cwd, from job
/// records and from orphaned directories under managed workspace roots.
/// Returns the worktrees plus the count of existing clone-isolated workspaces
/// (APFS/reflink/copy), which are real workspaces but not git worktrees.
///
/// Entries are deduplicated by canonical path: job records may spell a
/// workspace through a symlinked prefix (e.g. `/tmp` on macOS) while the
/// orphan scan sees the canonical form (`/private/tmp`) — without canonical
/// keys the same checkout would be listed twice and a stale-looking duplicate
/// could be removed out from under a running job.
fn collect_managed(cwd: &Path) -> (Vec<ManagedWorktree>, usize) {
    let mut worktrees: BTreeMap<PathBuf, ManagedWorktree> = BTreeMap::new();
    let mut roots: BTreeSet<PathBuf> = BTreeSet::new();
    let mut clone_workspaces = 0usize;
    let canonical_key = |path: &Path| fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
    for store in candidate_stores(cwd) {
        roots.insert(workspace_root_for(&store, cwd));
        for job in read_jobs(&store) {
            // Stores are per-project by construction, but session stores and a
            // shared JEDEN_TASK_STORE may hold jobs for other repositories;
            // only claim worktrees whose parent repo is this cwd.
            if !same_dir(&job.cwd, cwd) {
                continue;
            }
            roots.insert(workspace_root_for(&store, &job.cwd));
            let path = job.workspace.clone();
            if !path.is_dir() {
                continue;
            }
            if !is_worktree_checkout(&path) {
                if path.join(".git").is_dir() {
                    clone_workspaces += 1;
                }
                continue;
            }
            let (parent_repo, admin_dir) = parse_gitfile(&path)
                .map(|(repo, admin)| (Some(repo), Some(admin)))
                .unwrap_or((None, None));
            worktrees
                .entry(canonical_key(&path))
                .or_insert_with(|| ManagedWorktree {
                    branch: branch_of(&path),
                    created_ms: job.created_at,
                    job_id: Some(job.id.clone()),
                    status: Some(job.status.clone()),
                    pid: job.pid,
                    parent_repo: parent_repo.or_else(|| Some(job.cwd.clone())),
                    admin_dir,
                    path,
                });
        }
    }
    roots.insert(workspace_root_for(&default_store(cwd, None), cwd));
    // Orphaned workspaces: managed-root directories with a worktree gitfile
    // but no surviving job record.
    for root in roots {
        let Ok(entries) = fs::read_dir(&root) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if !path.is_dir() || !is_worktree_checkout(&path) {
                continue;
            }
            if worktrees.contains_key(&canonical_key(&path)) {
                continue;
            }
            let (parent_repo, admin_dir) = parse_gitfile(&path)
                .map(|(repo, admin)| (Some(repo), Some(admin)))
                .unwrap_or((None, None));
            worktrees
                .entry(canonical_key(&path))
                .or_insert_with(|| ManagedWorktree {
                    branch: branch_of(&path),
                    created_ms: modified_ms(&path),
                    job_id: None,
                    status: None,
                    pid: None,
                    parent_repo,
                    admin_dir,
                    path,
                });
        }
    }
    let mut ordered: Vec<ManagedWorktree> = worktrees.into_values().collect();
    ordered.sort_by(|a, b| {
        a.created_ms
            .cmp(&b.created_ms)
            .then_with(|| a.path.cmp(&b.path))
    });
    (ordered, clone_workspaces)
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

/// Fallback when the runtime manages no git worktrees here: the current
/// repository's own `git worktree list`.
fn repo_worktrees(cwd: &Path) -> Option<Vec<ManagedWorktree>> {
    let text = git(cwd, &["worktree", "list", "--porcelain"])?;
    let mut out = Vec::new();
    let mut path: Option<PathBuf> = None;
    let mut head = String::new();
    let mut branch = String::new();
    let mut detached = false;
    let mut flush = |path: &mut Option<PathBuf>,
                     head: &mut String,
                     branch: &mut String,
                     detached: &mut bool| {
        let Some(taken) = path.take() else { return };
        let label = if !branch.is_empty() {
            branch
                .strip_prefix("refs/heads/")
                .unwrap_or(branch)
                .to_string()
        } else if *detached && !head.is_empty() {
            format!("detached@{}", &head[..head.len().min(7)])
        } else {
            "-".to_string()
        };
        out.push(ManagedWorktree {
            created_ms: modified_ms(&taken),
            path: taken,
            branch: label,
            job_id: None,
            status: None,
            pid: None,
            parent_repo: None,
            admin_dir: None,
        });
        head.clear();
        branch.clear();
        *detached = false;
    };
    for line in text.lines() {
        if let Some(value) = line.strip_prefix("worktree ") {
            flush(&mut path, &mut head, &mut branch, &mut detached);
            path = Some(PathBuf::from(value));
        } else if let Some(value) = line.strip_prefix("HEAD ") {
            head = value.to_string();
        } else if let Some(value) = line.strip_prefix("branch ") {
            branch = value.to_string();
        } else if line == "detached" {
            detached = true;
        }
    }
    flush(&mut path, &mut head, &mut branch, &mut detached);
    Some(out)
}

fn render_list(args: &Args) -> String {
    let cwd = &args.cwd;
    let now = now_ms();
    let (managed, clone_workspaces) = collect_managed(cwd);
    if managed.is_empty() {
        let note = "no jeden-managed git worktrees found (the task runtime prefers clone-based isolation and only falls back to `git worktree add --detach`; clone-isolated workspaces are not worktrees)";
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
            "jeden-managed git worktrees for {} ({}):\n",
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

/// Canonicalized safety roots: a worktree may only be removed when it lives
/// inside one of the managed workspace roots and is neither the current
/// checkout nor the repository top level.
fn removal_allowed(path: &Path, roots: &[PathBuf], cwd: &Path, repo_top: Option<&Path>) -> bool {
    if !is_worktree_checkout(path) {
        return false;
    }
    let canonical = fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
    let cwd_canonical = fs::canonicalize(cwd).unwrap_or_else(|_| cwd.to_path_buf());
    if canonical == cwd_canonical {
        return false;
    }
    if let Some(top) = repo_top {
        let top = fs::canonicalize(top).unwrap_or_else(|_| top.to_path_buf());
        if canonical == top {
            return false;
        }
    }
    roots.iter().any(|root| canonical.starts_with(root))
}

fn render_clear(args: &Args, dry_run: bool) -> String {
    let cwd = &args.cwd;
    let now = now_ms();
    let (managed, _) = collect_managed(cwd);
    let repo_top = git(cwd, &["rev-parse", "--show-toplevel"]).map(PathBuf::from);
    let roots: Vec<PathBuf> = {
        let mut set = BTreeSet::new();
        for store in candidate_stores(cwd) {
            set.insert(workspace_root_for(&store, cwd));
        }
        for worktree in &managed {
            if let Some(parent) = worktree.path.parent() {
                set.insert(fs::canonicalize(parent).unwrap_or_else(|_| parent.to_path_buf()));
            }
        }
        set.into_iter().collect()
    };
    let mut removed: Vec<serde_json::Value> = Vec::new();
    let mut skipped: Vec<serde_json::Value> = Vec::new();
    let mut lines: Vec<String> = Vec::new();
    if managed.is_empty() {
        let message = "no jeden-managed git worktrees found; nothing to clear".to_string();
        if args.json {
            return serde_json::to_string_pretty(&serde_json::json!({
                "dryRun": dry_run,
                "removed": [],
                "skipped": [],
                "note": message,
            }))
            .unwrap_or_default()
                + "\n";
        }
        return format!("{message}\n");
    }
    for worktree in &managed {
        let row = to_row(worktree, now);
        if !worktree.stale() {
            let reason = match &worktree.job_id {
                Some(id) => format!("job {id} is still running"),
                None => "workspace still in use".to_string(),
            };
            skipped.push(serde_json::json!({"path": row.path, "reason": reason}));
            lines.push(format!(
                "  skipped {} · {} · {} ({reason})",
                row.path, row.branch, row.age
            ));
            continue;
        }
        if !removal_allowed(&worktree.path, &roots, cwd, repo_top.as_deref()) {
            let reason = "outside jeden-managed workspace roots; refusing to remove".to_string();
            skipped.push(serde_json::json!({"path": row.path, "reason": reason}));
            lines.push(format!(
                "  skipped {} · {} · {} ({reason})",
                row.path, row.branch, row.age
            ));
            continue;
        }
        if dry_run {
            removed.push(serde_json::json!({"path": row.path, "via": "dry-run"}));
            lines.push(format!(
                "  would remove {} · {} · {}",
                row.path, row.branch, row.age
            ));
            continue;
        }
        let via_git = worktree
            .parent_repo
            .as_ref()
            .map(|repo| {
                Command::new("git")
                    .args(["worktree", "remove"])
                    .arg(&worktree.path)
                    .current_dir(repo)
                    .output()
                    .map(|output| output.status.success())
                    .unwrap_or(false)
            })
            .unwrap_or(false);
        if via_git {
            removed.push(serde_json::json!({"path": row.path, "via": "git worktree remove"}));
            lines.push(format!(
                "  removed {} · {} · {} (via git worktree remove)",
                row.path, row.branch, row.age
            ));
            continue;
        }
        match fs::remove_dir_all(&worktree.path) {
            Ok(()) => {
                // Drop the stale administrative entry left in the parent repo.
                if let Some(admin) = &worktree.admin_dir {
                    if admin.starts_with(
                        worktree
                            .parent_repo
                            .as_ref()
                            .map(|repo| repo.join(".git"))
                            .unwrap_or_default(),
                    ) {
                        let _ = fs::remove_dir_all(admin);
                    }
                }
                removed.push(serde_json::json!({"path": row.path, "via": "rm -rf"}));
                lines.push(format!(
                    "  removed {} · {} · {} (via rm -rf)",
                    row.path, row.branch, row.age
                ));
            }
            Err(error) => {
                let reason = format!("removal failed: {error}");
                skipped.push(serde_json::json!({"path": row.path, "reason": reason}));
                lines.push(format!(
                    "  skipped {} · {} · {} ({reason})",
                    row.path, row.branch, row.age
                ));
            }
        }
    }
    if args.json {
        return serde_json::to_string_pretty(&serde_json::json!({
            "dryRun": dry_run,
            "removed": removed,
            "skipped": skipped,
        }))
        .unwrap_or_default()
            + "\n";
    }
    let header = if dry_run {
        format!(
            "jeden-managed git worktrees: {} stale of {} total (dry run; nothing removed)\n",
            removed.len(),
            managed.len()
        )
    } else {
        format!(
            "jeden-managed git worktrees: {} removed, {} kept\n",
            removed.len(),
            skipped.len()
        )
    };
    let mut out = header;
    for line in lines {
        out.push_str(&line);
        out.push('\n');
    }
    out
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
