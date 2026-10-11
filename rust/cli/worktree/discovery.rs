//! Finding the worktrees this command can act on: reading job records out of
//! every candidate scheduler store, scanning managed workspace roots for
//! orphans, and, when the runtime manages none, asking the repository itself.
//!
//! Split out of `cli/worktree.rs` because the operator's rule keeps every
//! source file at three hundred lines or fewer, and the write guard refuses
//! every edit to a file above that — including the header correction this
//! split unblocked.

use super::*;

/// The entries of `directory`; none only when it does not exist. A directory
/// that cannot be listed is refused: its job records would be missing, and
/// the workspaces they claim would look orphaned to `clear`.
fn listed(directory: &Path, what: &str) -> Result<Vec<PathBuf>, String> {
    let unreadable =
        |error: std::io::Error| format!("{what} {} cannot be listed: {error}", directory.display());
    let entries = match fs::read_dir(directory) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => return Err(unreadable(error)),
    };
    entries
        .map(|entry| entry.map(|entry| entry.path()).map_err(unreadable))
        .collect()
}

/// Every job record in `store`. A record that cannot be read or does not
/// parse is refused with its path, never skipped: skipped, its workspace had
/// no job and `clear` removed it from under the running job. A record that
/// vanished between listing and reading was retired and is passed over.
pub(super) fn read_jobs(store: &Path) -> Result<Vec<JobRecord>, String> {
    let mut jobs = Vec::new();
    for path in listed(&store.join("jobs"), "the job records in")? {
        if path.extension().and_then(|ext| ext.to_str()) != Some("json") {
            continue;
        }
        let bytes = match fs::read(&path) {
            Ok(bytes) => bytes,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
            Err(error) => {
                return Err(format!(
                    "the job record {} cannot be read: {error}",
                    path.display()
                ))
            }
        };
        let job = serde_json::from_slice::<JobRecord>(&bytes).map_err(|error| {
            format!(
                "the job record {} is not a job record: {error}",
                path.display()
            )
        })?;
        jobs.push(job);
    }
    Ok(jobs)
}

/// Scheduler stores that may hold job records relevant to `cwd`. A session
/// root that cannot be listed is refused, since its stores' jobs would be
/// missed.
pub(super) fn candidate_stores(cwd: &Path) -> Result<Vec<PathBuf>, String> {
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
    for session in listed(&session_root(), "the session root")? {
        push(session.join("task-runtime"), &mut seen, &mut stores);
    }
    Ok(stores)
}

/// `path` resolved, or as written when it does not resolve (a workspace
/// already gone still has a key).
pub(super) fn resolved(path: &Path) -> PathBuf {
    match fs::canonicalize(path) {
        Ok(canonical) => canonical,
        Err(_) => path.to_path_buf(),
    }
}

fn same_dir(a: &Path, b: &Path) -> bool {
    resolved(a) == resolved(b)
}

/// The parent repository and administrative directory a worktree's gitfile
/// names, when it names them.
fn gitfile(path: &Path) -> (Option<PathBuf>, Option<PathBuf>) {
    match parse_gitfile(path) {
        Some((repo, admin)) => (Some(repo), Some(admin)),
        None => (None, None),
    }
}

/// Collect every git worktree found for this cwd, from job records and from
/// orphaned directories under managed workspace roots. Returns the worktrees
/// plus the count of existing clone-isolated workspaces (APFS/reflink/copy),
/// which are real workspaces but not git worktrees.
///
/// Entries are deduplicated by canonical path: job records may spell a
/// workspace through a symlinked prefix (e.g. `/tmp` on macOS) while the
/// orphan scan sees the canonical form (`/private/tmp`) — without canonical
/// keys the same checkout would be listed twice and a stale-looking duplicate
/// could be removed out from under a running job.
pub(super) fn collect_managed(cwd: &Path) -> Result<(Vec<ManagedWorktree>, Vec<PathBuf>), String> {
    let mut worktrees: BTreeMap<PathBuf, ManagedWorktree> = BTreeMap::new();
    let mut roots: BTreeSet<PathBuf> = BTreeSet::new();
    let mut clone_workspaces = Vec::new();
    for store in candidate_stores(cwd)? {
        roots.insert(workspace_root_for(&store, cwd));
        for job in read_jobs(&store)? {
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
                    clone_workspaces.push(path);
                }
                continue;
            }
            let (parent_repo, admin_dir) = gitfile(&path);
            worktrees
                .entry(resolved(&path))
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
        for path in listed(&root, "the workspace root")? {
            if !path.is_dir() || !is_worktree_checkout(&path) {
                continue;
            }
            if worktrees.contains_key(&resolved(&path)) {
                continue;
            }
            let (parent_repo, admin_dir) = gitfile(&path);
            worktrees
                .entry(resolved(&path))
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
    Ok((ordered, clone_workspaces))
}

/// Used when no job record names a worktree here: the current repository's own
/// `git worktree list`, so worktrees made by hand or by another tool are still
/// reported and can still be cleaned up.
pub(super) fn repo_worktrees(cwd: &Path) -> Option<Vec<ManagedWorktree>> {
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
