//! Finding the worktrees this command can act on: reading job records out of
//! every candidate scheduler store, scanning managed workspace roots for
//! orphans, and, when the runtime manages none, asking the repository itself.
//!
//! Split out of `cli/worktree.rs` because the operator's rule keeps every
//! source file at three hundred lines or fewer, and the write guard refuses
//! every edit to a file above that — including the header correction this
//! split unblocked.

use super::*;

pub(super) fn read_jobs(store: &Path) -> Vec<JobRecord> {
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
pub(super) fn candidate_stores(cwd: &Path) -> Vec<PathBuf> {
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
pub(super) fn collect_managed(cwd: &Path) -> (Vec<ManagedWorktree>, usize) {
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
