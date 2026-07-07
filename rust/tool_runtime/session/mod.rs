use serde_json::{json, Value};
use std::fs;
use std::io::{self, Write};

use super::shared::{sha256_hex, string_input, u64_input, MAX_READ_BYTES};
use super::ToolRuntime;

mod memory;
mod todo;

pub(crate) use memory::memory_tool;
pub(crate) use todo::todo_tool;

pub(crate) fn save_artifact(runtime: &ToolRuntime<'_>, input: &Value) -> Result<Value, String> {
    let Some(dir) = runtime.artifact_dir else { return Err("save_artifact requires an active session artifact directory".into()); };
    let name = string_input(input, "name").unwrap_or_else(|| "artifact.txt".into());
    let content = string_input(input, "content").ok_or("save_artifact requires content")?;
    if name.contains('/') || name.contains("..") { return Err(format!("invalid artifact name: {name}")); }
    fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    let path = dir.join(&name);
    fs::write(&path, content.as_bytes()).map_err(|e| e.to_string())?;
    Ok(json!({"ok": true, "name": name, "path": path.display().to_string(), "bytes": content.len()}))
}

pub(crate) fn list_artifacts(runtime: &ToolRuntime<'_>) -> Result<Value, String> {
    let Some(dir) = runtime.artifact_dir else { return Err("list_artifacts requires an active session artifact directory".into()); };
    let mut artifacts = Vec::new();
    if let Ok(entries) = fs::read_dir(dir) {
        for entry in entries.flatten() {
            let meta = entry.metadata().map_err(|e| e.to_string())?;
            if meta.is_file() { artifacts.push(json!({"name": entry.file_name().to_string_lossy(), "bytes": meta.len()})); }
        }
    }
    Ok(json!({"ok": true, "artifacts": artifacts}))
}

pub(crate) fn read_artifact(runtime: &ToolRuntime<'_>, input: &Value) -> Result<Value, String> {
    let Some(dir) = runtime.artifact_dir else { return Err("read_artifact requires an active session artifact directory".into()); };
    let name = string_input(input, "name").ok_or("read_artifact requires name")?;
    let max_bytes = u64_input(input, "maxBytes", MAX_READ_BYTES).min(MAX_READ_BYTES) as usize;
    if name.contains('/') || name.contains("..") { return Err(format!("invalid artifact name: {name}")); }
    let path = dir.join(&name);
    let bytes = fs::read(&path).map_err(|e| e.to_string())?;
    let truncated = bytes.len() > max_bytes;
    let slice = &bytes[..bytes.len().min(max_bytes)];
    Ok(json!({"ok": true, "name": name, "bytes": bytes.len(), "truncated": truncated, "content": String::from_utf8_lossy(slice), "sha256": sha256_hex(&bytes)}))
}

pub(crate) fn ask_user(runtime: &ToolRuntime<'_>, input: &Value) -> Result<Value, String> {
    let question = string_input(input, "question").ok_or("ask_user requires question")?;
    if !runtime.interactive {
        return Err("ask_user is unavailable during a background turn; run without the interactive TUI or answer inline.".into());
    }
    let options = input.get("options").and_then(Value::as_array).map(|items| {
        items.iter().filter_map(Value::as_str).map(ToString::to_string).collect::<Vec<_>>()
    }).unwrap_or_default();
    eprintln!("\n[ask_user] {question}");
    if !options.is_empty() {
        for (index, option) in options.iter().enumerate() {
            eprintln!("  {}. {}", index + 1, option);
        }
    }
    eprint!("Answer: ");
    io::stderr().flush().map_err(|e| e.to_string())?;
    let mut answer = String::new();
    let bytes = io::stdin().read_line(&mut answer).map_err(|e| e.to_string())?;
    if bytes == 0 { return Err("ask_user requires interactive input".into()); }
    Ok(json!({"answer": answer.trim_end_matches(['\r', '\n']).to_string()}))
}
