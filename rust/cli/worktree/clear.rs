//! Removing stale worktrees, and refusing to remove anything else.
//!
//! Split out of `cli/worktree.rs` so that file fits the three-hundred-line
//! limit the operator's write guard enforces.
//!
//! Removal is deliberately narrow: a path is removed only when it is a real
//! worktree checkout, inside a managed workspace root, not the current
//! directory, and not a repository top level. Every refusal is reported with
//! its reason rather than passed over in silence.

use super::discovery::{candidate_stores, collect_managed};
use super::*;

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

pub(super) fn render_clear(args: &Args, dry_run: bool) -> String {
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
        let message = "no git worktrees found here; nothing to clear".to_string();
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
            "git worktrees here: {} stale of {} total (dry run; nothing removed)\n",
            removed.len(),
            managed.len()
        )
    } else {
        format!(
            "git worktrees here: {} removed, {} kept\n",
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
