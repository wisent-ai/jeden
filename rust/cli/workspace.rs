//! Adopt an existing local workspace without copying its files or session ledgers.
//!
//! The selected path is a user preference. Existing Jeden sessions remain in the
//! canonical session root and are associated by the `cwd` already sealed into
//! each session's `state.json`.

use serde::Serialize;
use serde_json::{json, Value};
use std::fs;
use std::path::{Path, PathBuf};

use super::config::{config_set_value, config_value_at, read_user_writable_config_strict, write_user_config};
use crate::{session_root, Args};

pub(crate) const DEFAULT_WORKSPACE_KEY: &str = "workspace.defaultPath";

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct WorkspaceSessions {
    pub(crate) accepted: usize,
    pub(crate) rejected: usize,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct WorkspaceReport {
    pub(crate) status: String,
    pub(crate) workspace: PathBuf,
    pub(crate) source: String,
    pub(crate) repository_root: Option<PathBuf>,
    pub(crate) sessions: WorkspaceSessions,
    pub(crate) configuration: String,
    pub(crate) imported: usize,
    pub(crate) unchanged: usize,
    pub(crate) conflicting: usize,
    pub(crate) rejected: usize,
}

impl WorkspaceReport {
    pub(crate) fn text(&self) -> String {
        let repository = self
            .repository_root
            .as_ref()
            .map(|path| path.display().to_string())
            .unwrap_or_else(|| "not a Git worktree".into());
        format!(
            "Workspace: {}\nStatus: {}\nSource: {} ({})\nSessions: {} accepted, {} rejected\nConfiguration: {}\nResult: {} imported, {} unchanged, {} conflicting, {} rejected\nThe working tree and session ledgers were not copied or changed. Future Jeden tasks use this workspace unless --cwd is supplied.",
            self.workspace.display(),
            self.status,
            self.source,
            repository,
            self.sessions.accepted,
            self.sessions.rejected,
            self.configuration,
            self.imported,
            self.unchanged,
            self.conflicting,
            self.rejected,
        )
    }

    pub(crate) fn value(&self) -> Value {
        serde_json::to_value(self).unwrap_or_else(|_| json!({"status": "invalid_response"}))
    }
}

fn resolve_input(path: &Path, base: &Path) -> PathBuf {
    if path.is_absolute() {
        path.to_path_buf()
    } else {
        base.join(path)
    }
}

fn repository_root(workspace: &Path) -> Option<PathBuf> {
    workspace
        .ancestors()
        .find(|candidate| candidate.join(".git").exists())
        .map(Path::to_path_buf)
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
    let value: Value = serde_json::from_slice(&bytes)
        .map_err(|error| format!("invalid existing Jeden configuration {}: {error}", path.display()))?;
    if !value.is_object() {
        return Err(format!(
            "invalid existing Jeden configuration {}: root must be an object",
            path.display()
        ));
    }
    Ok(format!("accepted existing {}", path.display()))
}

fn session_counts(workspace: &Path) -> Result<WorkspaceSessions, String> {
    let mut accepted = 0;
    let mut rejected = 0;
    let root = session_root();
    let entries = match fs::read_dir(&root) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Ok(WorkspaceSessions { accepted, rejected })
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
            Err(_) => {
                rejected += 1;
                continue;
            }
        };
        let state_path = entry.path().join("state.json");
        if !state_path.is_file() {
            continue;
        }
        let state = match fs::read(&state_path)
            .map_err(|error| error.to_string())
            .and_then(|bytes| serde_json::from_slice::<Value>(&bytes).map_err(|error| error.to_string()))
        {
            Ok(value) => value,
            Err(_) => {
                rejected += 1;
                continue;
            }
        };
        let Some(cwd) = state.get("cwd").and_then(Value::as_str) else {
            rejected += 1;
            continue;
        };
        let Ok(cwd) = Path::new(cwd).canonicalize() else {
            rejected += 1;
            continue;
        };
        if cwd.starts_with(workspace) {
            accepted += 1;
        }
    }
    Ok(WorkspaceSessions { accepted, rejected })
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
    let repository_root = repository_root(&workspace);
    let source = if repository_root.is_some() {
        "git-worktree"
    } else {
        "directory"
    };
    let sessions = session_counts(&workspace)?;
    Ok(WorkspaceReport {
        status: status.into(),
        workspace,
        source: source.into(),
        repository_root,
        rejected: sessions.rejected,
        sessions,
        configuration,
        imported: 0,
        unchanged: 0,
        conflicting: 0,
    })
}

pub(crate) fn configured_path() -> Result<Option<PathBuf>, String> {
    let config = read_user_writable_config_strict()?;
    let Some(value) = config_value_at(&config, DEFAULT_WORKSPACE_KEY) else {
        return Ok(None);
    };
    let path = value.as_str().ok_or_else(|| {
        format!(
            "{DEFAULT_WORKSPACE_KEY} in the user configuration must be a string"
        )
    })?;
    if path.trim().is_empty() {
        return Ok(None);
    }
    Ok(Some(PathBuf::from(path)))
}

pub(crate) fn effective_cwd(requested: &Path, explicit: bool) -> Result<PathBuf, String> {
    if explicit {
        return Ok(requested.to_path_buf());
    }
    let Some(configured) = configured_path()? else {
        return Ok(requested.to_path_buf());
    };
    inspect(&configured, requested, "selected")
        .map(|report| report.workspace)
        .map_err(|error| format!("adopted workspace is unavailable: {error}; use --cwd to override"))
}

pub(crate) fn status(base: &Path) -> Result<Option<WorkspaceReport>, String> {
    let Some(path) = configured_path()? else {
        return Ok(None);
    };
    inspect(&path, base, "adopted").map(Some)
}

pub(crate) fn adopt(path: &Path, base: &Path) -> Result<WorkspaceReport, String> {
    // Validate the complete source before opening the writable user config.
    let mut report = inspect(path, base, "adopted")?;
    let mut config = read_user_writable_config_strict()?;
    let already_selected = config_value_at(&config, DEFAULT_WORKSPACE_KEY)
        .and_then(Value::as_str)
        .is_some_and(|current| Path::new(current) == report.workspace.as_path());
    if already_selected {
        report.status = "unchanged".into();
        report.unchanged = 1;
        return Ok(report);
    }
    config_set_value(
        &mut config,
        DEFAULT_WORKSPACE_KEY,
        json!(report.workspace.display().to_string()),
    )?;
    let config_path = write_user_config(&config)?;
    report.imported = 1;
    report.configuration = format!(
        "{}; selected workspace persisted in {}",
        report.configuration,
        config_path.display()
    );
    Ok(report)
}

pub(crate) fn command(args: &Args) -> Result<String, String> {
    let verb = args.positionals.first().map(String::as_str).unwrap_or("status");
    let report = match verb {
        "status" => match status(&args.cwd)? {
            Some(report) => report,
            None => {
                return if args.json {
                    Ok("{\"status\":\"not_adopted\"}\n".into())
                } else {
                    Ok("No workspace is adopted. Run `jeden workspace adopt <path>`; the current working tree and existing session ledgers will be left in place.\n".into())
                }
            }
        },
        "discover" => {
            let path = args.positionals.get(1).map(PathBuf::from).unwrap_or_else(|| args.cwd.clone());
            inspect(&path, &args.cwd, "discovered")?
        }
        "adopt" => {
            let path = args
                .positionals
                .get(1)
                .ok_or("Usage: jeden workspace adopt <path> [--json]")?;
            adopt(Path::new(path), &args.cwd)?
        }
        _ => return Err("Usage: jeden workspace [status|discover [path]|adopt <path>] [--json]".into()),
    };
    if args.json {
        serde_json::to_string(&report)
            .map(|text| text + "\n")
            .map_err(|error| error.to_string())
    } else {
        Ok(report.text() + "\n")
    }
}
