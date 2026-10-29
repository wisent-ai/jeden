//! Bind an organization proposal to an admitted checkout before Pursuit starts.

use std::path::{Component, Path, PathBuf};

use super::Scope;
use crate::Args;

/// Only observed absence means "not a repository"; inspection failures stay failures.
pub(super) fn is_repository(path: &Path) -> Result<bool, String> {
    let metadata = match path.metadata() {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(false),
        Err(error) => {
            return Err(format!(
                "hillclimb cannot inspect repository directory {:?}: {error}",
                path.as_os_str()
            ))
        }
    };
    if !metadata.is_dir() {
        return Ok(false);
    }
    let marker = path.join(".git");
    match marker.metadata() {
        Ok(_) => Ok(true),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(error) => Err(format!(
            "hillclimb cannot inspect repository marker {:?}: {error}",
            marker.as_os_str()
        )),
    }
}

fn canonical(path: &Path) -> Result<PathBuf, String> {
    path.canonicalize().map_err(|error| {
        format!(
            "hillclimb cannot resolve the proposed workspace {}: {error}",
            path.display()
        )
    })
}

pub(super) fn arguments(
    args: &Args,
    scope: Scope,
    only: &[String],
    repository: Option<&str>,
) -> Result<Args, String> {
    let root = canonical(&args.cwd)?;
    let target = match scope {
        Scope::Repository => match repository {
            None => root.clone(),
            Some(name) => {
                let proposed = canonical(&root.join(name))?;
                if proposed != root {
                    return Err(format!(
                        "hillclimb chooser proposed repository {name:?} outside the repository scope {}; choose this checkout itself",
                        root.display()
                    ));
                }
                proposed
            }
        },
        Scope::Organization => {
            let name = repository.ok_or_else(|| {
                "hillclimb chooser named no repository for the organization round; name one immediate checkout before execution".to_owned()
            })?;
            let mut components = Path::new(name).components();
            match (components.next(), components.next()) {
                (Some(Component::Normal(_)), None) => {}
                _ => return Err(format!(
                    "hillclimb chooser repository {name:?} is not one immediate checkout name; absolute paths and nested paths are refused"
                )),
            }
            if !only.is_empty() && !only.iter().any(|allowed| allowed == name) {
                return Err(format!(
                    "hillclimb chooser repository {name:?} is not admitted by --only; admitted repositories: {only:?}"
                ));
            }
            let proposed = canonical(&root.join(name))?;
            if proposed.parent() != Some(root.as_path()) {
                return Err(format!(
                    "hillclimb chooser repository {name:?} resolves to {}, outside the immediate checkouts of {}",
                    proposed.display(), root.display()
                ));
            }
            proposed
        }
    };
    if !is_repository(&target)? {
        return Err(format!(
            "hillclimb chooser target {} is not a repository directory holding .git; no execution started",
            target.display()
        ));
    }
    let mut selected = args.clone();
    selected.cwd = target;
    selected.cwd_explicit = true;
    Ok(selected)
}
