use serde_json::{json, Value};
use std::path::PathBuf;
use std::process::Command;

use crate::slash::modes::current_model_route;
use crate::slash::SlashContext;
use crate::tools;

pub(crate) mod mcp;
pub(crate) mod memory;
pub(crate) mod ssh;
pub(crate) mod usage;

use memory::memory_file_path;

fn tool_values(context: &SlashContext<'_>) -> Vec<Value> {
    tools::list_tools(context.cwd)
        .into_iter()
        .map(|tool| json!({"name": tool.name, "description": tool.description, "input": {}}))
        .collect()
}

pub(crate) fn handle_doctor(context: &SlashContext<'_>) -> Result<String, String> {
    let all = tool_values(context);
    let report = json!({
        "ok": true,
        "cwd": context.cwd,
        "model": current_model_route(context),
        "checks": [
            {"id": "filesystem.cwd.readable", "ok": context.cwd.is_dir(), "fatal": true, "path": context.cwd},
            {"id": "tools.static.load", "ok": true, "fatal": false},
        ],
        "tools": {
            "total": all.len(),
        },
        "memory": {
            "backend": "local-jsonl",
            "path": memory_file_path(),
        },
    });
    serde_json::to_string_pretty(&report).map_err(|e| e.to_string())
}

/// Render recent release notes from the source repo's git history — the real
/// changelog for this package (no bundled CHANGELOG file exists).
pub(crate) fn handle_changelog() -> Result<String, String> {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let output = Command::new("git")
        .args(["log", "-20", "--pretty=format:%h  %ad  %s", "--date=short"])
        .current_dir(&root)
        .output()
        .map_err(|e| format!("git log failed: {e}"))?;
    if !output.status.success() {
        return Ok("No git history available for a changelog in this source tree.".into());
    }
    let log = String::from_utf8_lossy(&output.stdout).trim().to_string();
    if log.is_empty() {
        return Ok("No commits found for a changelog.".into());
    }
    Ok(format!("Recent changes (git history, {}):\n{}", root.display(), log))
}
