use serde_json::{json, Value};
use std::path::PathBuf;
use std::process::Command;

use crate::slash::modes::current_model_route;
use crate::slash::SlashContext;
use crate::tools;
use crate::tui::{PickerItem, PickerSpec};

pub(crate) mod agents;
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
    let memory_health = crate::memory::MemoryStore::open(memory_file_path())
        .and_then(|store| store.health())
        .unwrap_or_else(|error| {
            json!({
                "service": "memory",
                "healthy": false,
                "backend": "sqlite-wal-fts5",
                "path": memory_file_path(),
                "error": error,
            })
        });
    let ok = context.cwd.is_dir()
        && memory_health
            .get("healthy")
            .and_then(Value::as_bool)
            .unwrap_or(false);
    let report = json!({
        "ok": ok,
        "cwd": context.cwd,
        "model": current_model_route(context),
        "checks": [
            {"id": "filesystem.cwd.readable", "ok": context.cwd.is_dir(), "fatal": true, "path": context.cwd},
            {"id": "tools.static.load", "ok": true, "fatal": false},
        ],
        "tools": {
            "total": all.len(),
        },
        "memory": memory_health,
    });
    serde_json::to_string_pretty(&report).map_err(|e| e.to_string())
}

fn diagnostics_picker(title: &str, context: &SlashContext<'_>) -> PickerSpec {
    let tool_count = tools::list_tools(context.cwd).len();
    let model = current_model_route(context);
    let overview = PickerItem::action("Current runtime overview", "")
        .detail(format!(
            "Model {model}; {tool_count} tools; cwd {}",
            context.cwd.display()
        ))
        .badge("runtime")
        .disabled(true);
    let items = vec![
        PickerItem {
            command: None,
            ..overview
        },
        PickerItem::action("Inspect available tools", "/tools")
            .detail(format!("{tool_count} tools loaded for this workspace"))
            .badge("tools"),
        PickerItem::action("Inspect memory diagnostics", "/memory stats")
            .detail("Show SQLite/FTS5 integrity, record counts, and durable queue state")
            .badge("memory"),
        PickerItem::action("Inspect provider usage", "/usage status")
            .detail("Show local token and recorded cost accounting")
            .badge("usage"),
        PickerItem::action("Inspect extension status", "/extensions")
            .detail("Show effective configured extension state")
            .badge("extensions"),
    ];
    PickerSpec::new(title, items)
}

pub(crate) fn stats_picker(context: &SlashContext<'_>) -> PickerSpec {
    diagnostics_picker("Runtime statistics", context)
}

pub(crate) fn debug_picker(context: &SlashContext<'_>) -> PickerSpec {
    diagnostics_picker("Debug tools", context)
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
    Ok(format!(
        "Recent changes (git history, {}):\n{}",
        root.display(),
        log
    ))
}
