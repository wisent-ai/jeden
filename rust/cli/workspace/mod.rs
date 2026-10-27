//! Adopt an existing local workspace without copying its files or session ledgers.
//!
//! The selected path is a user preference. Existing Jeden sessions remain in the
//! canonical session root and are associated by the `cwd` already sealed into
//! each session's `state.json`.

use serde::Serialize;
use serde_json::{json, Value};
use std::path::{Path, PathBuf};

use super::config::{
    config_remove_value, config_set_value, config_value_at, read_user_writable_config_strict,
    write_user_config,
};
use crate::Args;

mod inspect;

pub(crate) use inspect::inspect;
pub(crate) const DEFAULT_WORKSPACE_KEY: &str = "workspace.defaultPath";

/// The sealed sessions that name a workspace, and the session directories
/// that could not be read, each with its cause.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct WorkspaceSessions {
    pub(crate) accepted: usize,
    pub(crate) rejected: usize,
    pub(crate) rejections: Vec<String>,
}

impl WorkspaceSessions {
    pub(crate) fn from_outcomes(accepted: &[PathBuf], rejections: Vec<String>) -> Self {
        Self {
            accepted: accepted.len(),
            rejected: rejections.len(),
            rejections,
        }
    }
}

/// What an adoption did with the one workspace it was given. A workspace that
/// was only looked at was neither imported, nor found unchanged, nor in conflict.
#[derive(Debug, Clone, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct WorkspaceImports {
    pub(crate) imported: usize,
    pub(crate) unchanged: usize,
    pub(crate) conflicting: usize,
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
    #[serde(flatten)]
    pub(crate) imports: WorkspaceImports,
    pub(crate) rejected: usize,
}

impl WorkspaceReport {
    pub(crate) fn text(&self) -> String {
        let repository = match &self.repository_root {
            Some(path) => path.display().to_string(),
            None => "not in a Git repository".into(),
        };
        format!(
            "Workspace: {}\nStatus: {}\nSource: {} ({})\nSessions: {} accepted, {} rejected\nConfiguration: {}\nResult: {} imported, {} unchanged, {} conflicting, {} rejected\nThe working tree and session ledgers were not copied or changed. Future Jeden tasks use this workspace unless --cwd is supplied.",
            self.workspace.display(),
            self.status,
            self.source,
            repository,
            self.sessions.accepted,
            self.sessions.rejected,
            self.configuration,
            self.imports.imported,
            self.imports.unchanged,
            self.imports.conflicting,
            self.rejected,
        )
    }

    /// The report as JSON; a workspace path that is not UTF-8 cannot be one.
    pub(crate) fn value(&self) -> Result<Value, String> {
        serde_json::to_value(self).map_err(|error| {
            format!(
                "the report on workspace {} cannot be written as JSON: {error}",
                self.workspace.display()
            )
        })
    }

    /// A report for a workspace that was only looked at: inspection reads a
    /// directory and imports nothing from it. Adoption is the operation that
    /// moves sessions, and it fills the imports itself.
    pub(crate) fn observed(
        status: &str,
        workspace: PathBuf,
        source: &str,
        repository_root: Option<PathBuf>,
        configuration: String,
        sessions: WorkspaceSessions,
    ) -> Self {
        Self {
            status: status.into(),
            workspace,
            source: source.into(),
            repository_root,
            rejected: sessions.rejected,
            sessions,
            configuration,
            imports: WorkspaceImports::default(),
        }
    }
}

pub(crate) fn configured_path() -> Result<Option<PathBuf>, String> {
    let config = read_user_writable_config_strict()?;
    let Some(value) = config_value_at(&config, DEFAULT_WORKSPACE_KEY) else {
        return Ok(None);
    };
    let path = value.as_str().ok_or_else(|| {
        format!("{DEFAULT_WORKSPACE_KEY} in the user configuration must be a string")
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
        .map_err(|error| {
            format!("adopted workspace is unavailable: {error}; use --cwd to override")
        })
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
        report.imports.unchanged = usize::from(crate::tally::STEP);
        return Ok(report);
    }
    config_set_value(
        &mut config,
        DEFAULT_WORKSPACE_KEY,
        json!(report.workspace.display().to_string()),
    )?;
    let config_path = write_user_config(&config)?;
    report.imports.imported = usize::from(crate::tally::STEP);
    report.configuration = format!(
        "{}; selected workspace persisted in {}",
        report.configuration,
        config_path.display()
    );
    Ok(report)
}

/// The inverse of [`adopt`]: the selected workspace leaves the user
/// configuration, so tasks run in the current directory again. Nothing in
/// the workspace or its session ledgers is touched. Returns the path that was
/// selected, or `None` when none was, so a repeated forget changes nothing.
pub(crate) fn forget() -> Result<Option<PathBuf>, String> {
    let previous = configured_path()?;
    if previous.is_none() {
        return Ok(None);
    }
    let mut config = read_user_writable_config_strict()?;
    config_remove_value(&mut config, DEFAULT_WORKSPACE_KEY)?;
    write_user_config(&config)?;
    Ok(previous)
}

pub(crate) fn command(args: &Args) -> Result<String, String> {
    // Without a verb the command reports the adopted workspace's status.
    let (verb, operands) = match args.positionals.split_first() {
        Some((verb, operands)) => (verb.as_str(), operands),
        None => ("status", &[][..]),
    };
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
            // Without a path, the working directory is the one looked at.
            let path = match operands.first() {
                Some(path) => PathBuf::from(path),
                None => args.cwd.clone(),
            };
            inspect(&path, &args.cwd, "discovered")?
        }
        "adopt" => {
            let path = operands
                .first()
                .ok_or("Usage: jeden workspace adopt <path> [--json]")?;
            adopt(Path::new(path), &args.cwd)?
        }
        "forget" => {
            let previous = forget()?;
            return Ok(match (args.json, previous) {
                (true, previous) => format!(
                    "{}\n",
                    json!({"status": if previous.is_some() { "forgotten" } else { "not_adopted" }, "workspace": previous})
                ),
                (false, Some(path)) => format!(
                    "Forgot the adopted workspace {}; tasks run in the current directory unless --cwd is supplied. Its files and sessions were not changed.\n",
                    path.display()
                ),
                (false, None) => "No workspace is adopted; nothing changed.\n".into(),
            });
        }
        _ => {
            return Err(
                "Usage: jeden workspace [status|discover [path]|adopt <path>|forget] [--json]"
                    .into(),
            )
        }
    };
    if args.json {
        serde_json::to_string(&report)
            .map(|text| text + "\n")
            .map_err(|error| error.to_string())
    } else {
        Ok(report.text() + "\n")
    }
}
