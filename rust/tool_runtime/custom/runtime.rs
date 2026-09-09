use std::fs;
use std::path::{Path, PathBuf};

use serde_json::Value;

use crate::tool_runtime::ToolRuntime;

/// Where the extension host leaves a reload request.
///
/// An extension tool runs in a short-lived host process, so it cannot reach
/// the registry this session holds in memory. `context.requestReload()` writes
/// this marker and the session honours it here, immediately after the tool
/// answers. Before 2026-09-09 there was no such path at all: a tool could
/// install extension material and then only report that the runtime it had
/// just installed was not the runtime it was running, and the sole way to load
/// it was a slash command typed by a person.
const RELOAD_REQUEST: &str = ".jeden/runtime/extensions/reload-request.json";

fn reload_request_path(cwd: &Path) -> PathBuf {
    cwd.join(RELOAD_REQUEST)
}

/// Consume a pending request, so one tool call reloads at most once.
fn take_reload_request(cwd: &Path) -> bool {
    let path = reload_request_path(cwd);
    if !path.is_file() {
        return false;
    }
    let _ = fs::remove_file(&path);
    true
}

pub(crate) fn custom_tool(
    runtime: &ToolRuntime<'_>,
    tool: &str,
    input: &Value,
) -> Result<Value, String> {
    let _ = take_reload_request(runtime.cwd);
    let answer = crate::hooks::execute_extension_tool(
        runtime.cwd,
        runtime.artifact_dir,
        &runtime.operation,
        runtime.allow_write,
        runtime.allow_command,
        tool,
        input,
    )?
    .ok_or_else(|| format!("custom or extension tool not found: {tool}"))?;
    if !take_reload_request(runtime.cwd) {
        return Ok(answer);
    }
    // The reload runs in this process because a registry rebuilt anywhere else
    // changes nothing about what this session dispatches. Its outcome travels
    // beside the tool's own answer rather than replacing it, so a tool that
    // asked for a reload can never look successful while the reload failed.
    match crate::hooks::reload_extensions(runtime.cwd) {
        Ok(report) => Ok(serde_json::json!({
            "result": answer,
            "extensionReload": {
                "reloaded": true,
                "generation": report.generation,
                "activeExtensions": report.active_extensions,
                "tools": report.tools,
                "hooks": report.hooks,
            },
        })),
        Err(error) => Ok(serde_json::json!({
            "result": answer,
            "extensionReload": { "reloaded": false, "error": error },
        })),
    }
}
