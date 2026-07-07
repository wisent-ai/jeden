use serde_json::{json, Value};
use std::fs;

use super::shared::{jail_path, string_input, u64_input};
use super::ToolRuntime;

mod archive;
mod document;
mod files;
mod sqlite;

pub(crate) use archive::read_archive;
pub(crate) use document::{fetch_readable_url, read_document};
pub(crate) use files::{read_binary_file, read_file, read_image};
pub(crate) use sqlite::read_sqlite;

pub(crate) fn list_dir(runtime: &ToolRuntime<'_>, input: &Value) -> Result<Value, String> {
    let path = string_input(input, "path").unwrap_or_else(|| ".".into());
    let limit = u64_input(input, "limit", 200) as usize;
    let dir = jail_path(runtime.cwd, &path)?;
    let mut entries = Vec::new();
    for entry in fs::read_dir(&dir).map_err(|e| e.to_string())?.flatten().take(limit) {
        let meta = entry.metadata().map_err(|e| e.to_string())?;
        entries.push(json!({
            "name": entry.file_name().to_string_lossy(),
            "path": entry.path().strip_prefix(runtime.cwd).unwrap_or(entry.path().as_path()).display().to_string(),
            "type": if meta.is_dir() { "directory" } else if meta.is_file() { "file" } else { "other" },
            "size": if meta.is_file() { meta.len() as i64 } else { 0 },
        }));
    }
    Ok(json!({"ok": true, "path": path, "entries": entries}))
}
