//! The workspace requests a client can make, through the same product code
//! the command line uses.

use super::wire::string_param;
use serde_json::json;
use serde_json::Value;
use std::path::PathBuf;

/// The working directory's workspace, or `{"status": "not_adopted"}` when
/// none is adopted there, which is the whole answer for that case.
pub(crate) fn workspace_status() -> Result<Value, (&'static str, String)> {
    let cwd = std::env::current_dir().map_err(|error| ("storage", error.to_string()))?;
    match crate::cli::workspace::status(&cwd).map_err(|error| ("invalid_workspace", error))? {
        Some(report) => report.value().map_err(|error| ("storage", error)),
        None => Ok(json!({"status": "not_adopted"})),
    }
}

/// What a workspace at `path` would be, `path` being the working directory
/// when the request names none; a `path` that is not text is refused.
pub(crate) fn workspace_discover(params: &Value) -> Result<Value, (&'static str, String)> {
    let cwd = std::env::current_dir().map_err(|error| ("storage", error.to_string()))?;
    let path = match params.get("path") {
        None => cwd.clone(),
        Some(Value::String(path)) => PathBuf::from(path),
        Some(other) => return Err(("invalid_request", format!("path is {other}, not a path"))),
    };
    let report = crate::cli::workspace::inspect(&path, &cwd, "discovered")
        .map_err(|error| ("invalid_workspace", error))?;
    report.value().map_err(|error| ("storage", error))
}

pub(crate) fn workspace_adopt(params: &Value) -> Result<Value, (&'static str, String)> {
    let cwd = std::env::current_dir().map_err(|error| ("storage", error.to_string()))?;
    let path = string_param(params, "path")
        .map(PathBuf::from)
        .map_err(|error| ("invalid_request", error))?;
    let report = crate::cli::workspace::adopt(&path, &cwd)
        .map_err(|error| ("invalid_workspace", error))?;
    report.value().map_err(|error| ("storage", error))
}

/// `session/import` takes the same paths the CLI does: transcript files of
/// another harness, or directories to scan. A missing or empty `paths` is a
/// request error; a path that cannot be imported is a failure row in the
/// result, so one bad file never hides the rest.
pub(crate) fn import_sessions(params: &Value) -> Result<Value, (&'static str, String)> {
    let Some(listed) = params.get("paths").and_then(Value::as_array) else {
        return Err((
            "invalid_params",
            "paths must be a non-empty array of strings".into(),
        ));
    };
    // An entry that is not a path is refused by its place, not left out:
    // a dropped entry would be an import nobody asked to skip.
    let paths = listed
        .iter()
        .enumerate()
        .map(|(at, path)| {
            path.as_str().map(PathBuf::from).ok_or_else(|| {
                (
                    "invalid_params",
                    format!("paths[{at}] is {path}, not a path string"),
                )
            })
        })
        .collect::<Result<Vec<PathBuf>, _>>()?;
    if paths.is_empty() {
        return Err((
            "invalid_params",
            "paths must be a non-empty array of strings".into(),
        ));
    }
    let refresh = match params.get("refresh") {
        None | Some(Value::Null) => false,
        Some(Value::Bool(refresh)) => *refresh,
        Some(other) => {
            return Err((
                "invalid_params",
                format!("refresh must be true or false, not {other}"),
            ))
        }
    };
    crate::session_import::import_paths(&paths, refresh).map_err(|error| ("import_error", error))
}
