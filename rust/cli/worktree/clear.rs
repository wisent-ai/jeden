//! Removing stale worktrees, and refusing to remove anything else.
//!
//! Split out of `cli/worktree.rs` so that file fits the three-hundred-line
//! limit the operator's write guard enforces.
//!
//! Removal is deliberately narrow: a path is removed only when it is a real
//! worktree checkout, inside a managed workspace root, not the current
//! directory, and not a repository top level. Every refusal is reported with
//! its reason rather than passed over in silence.

use super::discovery::{candidate_stores, collect_managed, resolved};
use super::*;

/// Canonicalized safety roots: a worktree may only be removed when it lives
/// inside one of the managed workspace roots and is neither the current
/// checkout nor the repository top level.
fn removal_allowed(path: &Path, roots: &[PathBuf], cwd: &Path, repo_top: Option<&Path>) -> bool {
    if !is_worktree_checkout(path) {
        return false;
    }
    let canonical = resolved(path);
    let cwd_canonical = resolved(cwd);
    if canonical == cwd_canonical {
        return false;
    }
    if let Some(top) = repo_top {
        let top = resolved(top);
        if canonical == top {
            return false;
        }
    }
    roots.iter().any(|root| canonical.starts_with(root))
}

/// Remove the stale worktrees. A job record, store or workspace root that
/// cannot be read refuses the whole clear before anything is removed: its
/// workspace would look orphaned and be removed from under its job.
pub(super) fn render_clear(args: &Args, dry_run: bool) -> Result<String, String> {
    let cwd = &args.cwd;
    let now = now_ms();
    let (managed, _) = collect_managed(cwd)?;
    let repo_top = git(cwd, &["rev-parse", "--show-toplevel"]).map(PathBuf::from);
    let roots: Vec<PathBuf> = {
        let mut set = BTreeSet::new();
        for store in candidate_stores(cwd)? {
            set.insert(workspace_root_for(&store, cwd));
        }
        for worktree in &managed {
            if let Some(parent) = worktree.path.parent() {
                set.insert(resolved(parent));
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
            return printed(&serde_json::json!({
                "dryRun": dry_run,
                "removed": [],
                "skipped": [],
                "note": message,
            }));
        }
        return Ok(format!("{message}\n"));
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
        // `git worktree remove` refuses a checkout with modified or untracked
        // files; that refusal is the reason it stays, never a cue to delete
        // it another way. A checkout no repository claims is deleted only
        // when `git status` in it answers and reports nothing uncommitted.
        let removal = match worktree.parent_repo.as_ref() {
            Some(repo) => match Command::new("git")
                .args(["worktree", "remove"])
                .arg(&worktree.path)
                .current_dir(repo)
                .output()
            {
                Ok(output) if output.status.success() => Ok("git worktree remove"),
                Ok(output) => Err(format!(
                    "git worktree remove refused: {}",
                    String::from_utf8_lossy(&output.stderr).trim()
                )),
                Err(error) => Err(format!(
                    "git could not be run in {}: {error}",
                    repo.display()
                )),
            },
            None => match git(&worktree.path, &["status", "--porcelain"]) {
                Some(status) if status.is_empty() => fs::remove_dir_all(&worktree.path)
                    .map_err(|error| {
                        format!("removal of {} failed: {error}", worktree.path.display())
                    })
                    .and_then(|()| {
                        if let Some(admin) = &worktree.admin_dir {
                            fs::remove_dir_all(admin).map_err(|error| {
                                format!(
                                    "checkout {} was removed, but removing its administrative directory {} failed: {error}",
                                    worktree.path.display(),
                                    admin.display()
                                )
                            })?;
                        }
                        Ok("rm -rf (clean checkout, no parent repository)")
                    }),
                Some(status) => Err(format!(
                    "uncommitted changes in a checkout no repository claims: {}",
                    status.lines().collect::<Vec<_>>().join("; ")
                )),
                None => {
                    Err("git status does not answer in it, so its changes are unknown".to_string())
                }
            },
        };
        match removal {
            Ok(via) => {
                removed.push(serde_json::json!({"path": row.path, "via": via}));
                lines.push(format!(
                    "  removed {} · {} · {} (via {via})",
                    row.path, row.branch, row.age
                ));
            }
            Err(reason) => {
                skipped.push(serde_json::json!({"path": row.path, "reason": reason}));
                lines.push(format!(
                    "  skipped {} · {} · {} ({reason})",
                    row.path, row.branch, row.age
                ));
            }
        }
    }
    if args.json {
        return printed(&serde_json::json!({
            "dryRun": dry_run,
            "removed": removed,
            "skipped": skipped,
        }));
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
    Ok(out)
}
