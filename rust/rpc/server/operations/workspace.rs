//! The workspace requests a client can make, through the same product code
//! the command line uses.
//!
//! Split out of `rpc/server/operations.rs`, which had grown past the module
//! line cap.

use super::wire::string_param;
use serde_json::Value;
use std::path::PathBuf;
use serde_json::json;

pub(crate) fn workspace_status() -> Result<Value, (&'static str, String)> {
    let cwd = std::env::current_dir().map_err(|error| ("storage", error.to_string()))?;
    crate::cli::workspace::status(&cwd)
        .map(|report| {
            report
                .map(|report| report.value())
                .unwrap_or_else(|| json!({"status": "not_adopted"}))
        })
        .map_err(|error| ("invalid_workspace", error))
}

pub(crate) fn workspace_discover(params: &Value) -> Result<Value, (&'static str, String)> {
    let cwd = std::env::current_dir().map_err(|error| ("storage", error.to_string()))?;
    let path = params
        .get("path")
        .and_then(Value::as_str)
        .map(PathBuf::from)
        .unwrap_or_else(|| cwd.clone());
    crate::cli::workspace::inspect(&path, &cwd, "discovered")
        .map(|report| report.value())
        .map_err(|error| ("invalid_workspace", error))
}

pub(crate) fn workspace_adopt(params: &Value) -> Result<Value, (&'static str, String)> {
    let cwd = std::env::current_dir().map_err(|error| ("storage", error.to_string()))?;
    let path = string_param(params, "path")
        .map(PathBuf::from)
        .map_err(|error| ("invalid_request", error))?;
    crate::cli::workspace::adopt(&path, &cwd)
        .map(|report| report.value())
        .map_err(|error| ("invalid_workspace", error))
}

/// `session/import` takes the same paths the CLI does: transcript files of
/// another harness, or directories to scan. A missing or empty `paths` is a
/// request error; a path that cannot be imported is a failure row in the
/// result, so one bad file never hides the rest.
pub(crate) fn import_sessions(params: &Value) -> Result<Value, (&'static str, String)> {
    let paths: Vec<PathBuf> = params
        .get("paths")
        .and_then(Value::as_array)
        .map(|paths| {
            paths
                .iter()
                .filter_map(Value::as_str)
                .map(PathBuf::from)
                .collect()
        })
        .unwrap_or_default();
    if paths.is_empty() {
        return Err((
            "invalid_params",
            "paths must be a non-empty array of strings".into(),
        ));
    }
    let refresh = params
        .get("refresh")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    crate::session_import::import_paths(&paths, refresh).map_err(|error| ("import_error", error))
}
