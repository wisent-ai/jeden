use super::{store::identifier, Request};
use std::{
    collections::BTreeMap,
    path::Path,
    process::{Command, Stdio},
};

fn git(cwd: &Path, args: &[&str]) -> Result<String, String> {
    let mut command = Command::new("git");
    command.arg("-C").arg(cwd).args(args).stdin(Stdio::null());
    let output =
        stado_wait::output(&mut command).map_err(|e| format!("git {}: {e}", args.join(" ")))?;
    if !output.status.success() {
        return Err(format!(
            "git {} in {}: exit {:?}: {}",
            args.join(" "),
            cwd.display(),
            output.status.code(),
            String::from_utf8_lossy(&output.stderr)
        ));
    }
    String::from_utf8(output.stdout)
        .map(|text| text.trim().to_owned())
        .map_err(|e| e.to_string())
}

/// `owner/name` split into its two identifiers, or `None` when it is not
/// exactly two of them.
fn owner_name(identity: &str) -> Option<(&str, &str)> {
    let (owner, name) = identity.split_once('/')?;
    (identifier(owner) && identifier(name)).then_some((owner, name))
}

/// The `owner/name` an origin URL names: the path after the host, in an
/// `https://host/owner/name` or `git@host:owner/name` address, without `.git`.
pub(super) fn repository(cwd: &Path) -> Result<String, String> {
    let url = git(cwd, &["config", "--get", "remote.origin.url"])?;
    let path = match url.split_once("://") {
        Some((_, located)) => located.split_once('/').map(|(_, path)| path),
        None => url.split_once(':').map(|(_, path)| path),
    }
    .ok_or_else(|| format!("origin {url} names no repository path"))?;
    let identity = path.trim_end_matches(".git");
    if owner_name(identity).is_none() {
        return Err(format!(
            "origin {url} names no owner/name repository identity"
        ));
    }
    Ok(identity.to_owned())
}

pub(super) fn snapshot(
    request: &Request,
    require_clean: bool,
) -> Result<BTreeMap<String, String>, String> {
    if Path::new(&git(&request.cwd, &["rev-parse", "--show-toplevel"])?) != request.cwd {
        return Err(
            "request cwd must name the canonical repository root, not a subdirectory".into(),
        );
    }
    let owner = repository(&request.cwd)?;
    let root = request
        .cwd
        .parent()
        .ok_or("canonical checkout has no workspace parent")?;
    let mut repositories = request.repositories.clone();
    if !repositories.contains(&owner) {
        repositories.push(owner);
    }
    let mut revisions = BTreeMap::new();
    for identity in repositories {
        let (_, name) =
            owner_name(&identity).ok_or("request repository must be an owner/name identity")?;
        let cwd = root
            .join(name)
            .canonicalize()
            .map_err(|e| format!("canonical checkout {identity}: {e}"))?;
        if cwd.parent() != Some(root) || repository(&cwd)? != identity {
            return Err(format!("{identity} does not match its canonical checkout"));
        }
        if git(&cwd, &["branch", "--show-current"])? != "main" {
            return Err(format!("{identity} is not on main; no branch was changed"));
        }
        let listing = git(&cwd, &["worktree", "list", "--porcelain"])?;
        let checkouts: Vec<&str> = listing
            .lines()
            .filter(|line| line.starts_with("worktree "))
            .collect();
        if !matches!(checkouts.as_slice(), [_]) {
            return Err(format!(
                "{identity} has multiple checkouts; none was changed"
            ));
        }
        if require_clean && !git(&cwd, &["status", "--porcelain"])?.is_empty() {
            return Err(format!(
                "{identity} has uncommitted changes; the autonomous request did not adopt them"
            ));
        }
        let revision = git(&cwd, &["rev-parse", "--verify", "HEAD"])?;
        if revisions.insert(identity, revision).is_some() {
            return Err("duplicate repository in request scope".into());
        }
    }
    Ok(revisions)
}

pub(super) fn pushed(
    request: &Request,
    revisions: &BTreeMap<String, String>,
) -> Result<(), String> {
    let root = request
        .cwd
        .parent()
        .ok_or("canonical checkout has no parent")?;
    for (repository, revision) in revisions {
        let (_, name) = owner_name(repository).ok_or("invalid repository identity")?;
        let remote = git(
            &root.join(name),
            &["ls-remote", "origin", "refs/heads/main"],
        )?;
        if remote.split_whitespace().next() != Some(revision.as_str()) {
            return Err(format!(
                "{repository}: the reviewed {revision} is not the observed origin/main revision"
            ));
        }
    }
    Ok(())
}
