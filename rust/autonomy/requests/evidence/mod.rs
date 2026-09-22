use super::{store::identifier, Request};
use std::{
    collections::BTreeMap,
    path::Path,
    process::{Command, Stdio},
};

fn git(cwd: &Path, args: &[&str]) -> Result<String, String> {
    let output = Command::new("git")
        .arg("-C")
        .arg(cwd)
        .args(args)
        .stdin(Stdio::null())
        .output()
        .map_err(|e| format!("git {}: {e}", args.join(" ")))?;
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

pub(super) fn repository(cwd: &Path) -> Result<String, String> {
    let url = git(cwd, &["config", "--get", "remote.origin.url"])?;
    let name = url
        .strip_prefix("https://github.com/")
        .or_else(|| url.strip_prefix("git@github.com:"))
        .ok_or("pursuit source binding requires the canonical GitHub origin")?
        .trim_end_matches(".git");
    let parts = name.split('/').collect::<Vec<_>>();
    if parts.len() != 2 || !parts.iter().all(|part| identifier(part)) {
        return Err("invalid repository identity in origin".into());
    }
    Ok(name.to_owned())
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
    for name in repositories {
        let parts = name.split('/').collect::<Vec<_>>();
        if parts.len() != 2 || !parts.iter().all(|part| identifier(part)) {
            return Err("request repository must be an owner/name identity".into());
        }
        let cwd = root
            .join(parts[1])
            .canonicalize()
            .map_err(|e| format!("canonical checkout {name}: {e}"))?;
        if cwd.parent() != Some(root) || repository(&cwd)? != name {
            return Err(format!("{name} does not match its canonical checkout"));
        }
        if git(&cwd, &["branch", "--show-current"])? != "main" {
            return Err(format!("{name} is not on main; no branch was changed"));
        }
        if git(&cwd, &["worktree", "list", "--porcelain"])?
            .lines()
            .filter(|line| line.starts_with("worktree "))
            .count()
            != 1
        {
            return Err(format!("{name} has multiple checkouts; none was changed"));
        }
        if require_clean && !git(&cwd, &["status", "--porcelain"])?.is_empty() {
            return Err(format!(
                "{name} has uncommitted changes; the autonomous request did not adopt them"
            ));
        }
        let revision = git(&cwd, &["rev-parse", "--verify", "HEAD"])?;
        if revisions.insert(name, revision).is_some() {
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
        let (_, name) = repository
            .split_once('/')
            .ok_or("invalid repository identity")?;
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
