use serde_json::{json, Value};
use std::fs;
use std::io::{self, Write};
use std::path::Path;

use super::shared::{sha256_hex, string_input, u64_input, MAX_READ_BYTES};
use super::ToolRuntime;

mod context;
mod memory;
mod todo;

pub(crate) use context::context_tool;
pub(crate) use memory::memory_tool;
pub(crate) use todo::todo_tool;

fn active_roadmap_item(cwd: &std::path::Path) -> Option<String> {
    fs::read_to_string(cwd.join(".jeden/mode-state.json"))
        .ok()
        .and_then(|text| serde_json::from_str::<Value>(&text).ok())
        .and_then(|state| {
            state
                .get("activeRoadmapItem")
                .and_then(Value::as_str)
                .map(str::to_string)
        })
}

fn session_roadmap_item(dir: &std::path::Path) -> Option<String> {
    fs::read_to_string(dir.parent()?.join("roadmap-item.json"))
        .ok()
        .and_then(|text| serde_json::from_str::<Value>(&text).ok())
        .and_then(|metadata| {
            metadata
                .get("itemId")
                .and_then(Value::as_str)
                .map(str::to_string)
        })
}

fn artifact_roadmap_item(runtime: &ToolRuntime<'_>, dir: &std::path::Path) -> Option<String> {
    session_roadmap_item(dir).or_else(|| active_roadmap_item(runtime.cwd))
}
pub(crate) fn save_artifact(runtime: &ToolRuntime<'_>, input: &Value) -> Result<Value, String> {
    let Some(dir) = runtime.artifact_dir else {
        return Err("save_artifact requires an active session artifact directory".into());
    };
    let name = string_input(input, "name").unwrap_or_else(|| "artifact.txt".into());
    let content = string_input(input, "content").ok_or("save_artifact requires content")?;
    if name.contains('/') || name.contains("..") {
        return Err(format!("invalid artifact name: {name}"));
    }
    fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    let path = dir.join(&name);
    fs::write(&path, content.as_bytes()).map_err(|e| e.to_string())?;
    Ok(json!({
        "ok": true,
        "name": name,
        "path": path.display().to_string(),
        "bytes": content.len(),
        "roadmapItem": artifact_roadmap_item(runtime, dir)
    }))
}

pub(crate) fn list_artifacts(runtime: &ToolRuntime<'_>) -> Result<Value, String> {
    let Some(dir) = runtime.artifact_dir else {
        return Err("list_artifacts requires an active session artifact directory".into());
    };
    let mut artifacts = Vec::new();
    if let Ok(entries) = fs::read_dir(dir) {
        for entry in entries.flatten() {
            let meta = entry.metadata().map_err(|e| e.to_string())?;
            if meta.is_file() {
                artifacts.push(
                    json!({"name": entry.file_name().to_string_lossy(), "bytes": meta.len()}),
                );
            }
        }
    }
    Ok(json!({
        "ok": true,
        "roadmapItem": artifact_roadmap_item(runtime, dir),
        "artifacts": artifacts
    }))
}

pub(crate) fn read_artifact(runtime: &ToolRuntime<'_>, input: &Value) -> Result<Value, String> {
    let Some(dir) = runtime.artifact_dir else {
        return Err("read_artifact requires an active session artifact directory".into());
    };
    let name = string_input(input, "name").ok_or("read_artifact requires name")?;
    let max_bytes = u64_input(input, "maxBytes", MAX_READ_BYTES).min(MAX_READ_BYTES) as usize;
    if name.contains('/') || name.contains("..") {
        return Err(format!("invalid artifact name: {name}"));
    }
    let path = dir.join(&name);
    let bytes = fs::read(&path).map_err(|e| e.to_string())?;
    let truncated = bytes.len() > max_bytes;
    let slice = &bytes[..bytes.len().min(max_bytes)];
    Ok(json!({
        "ok": true,
        "name": name,
        "bytes": bytes.len(),
        "truncated": truncated,
        "content": String::from_utf8_lossy(slice),
        "sha256": sha256_hex(&bytes),
        "roadmapItem": artifact_roadmap_item(runtime, dir)
    }))
}

pub(crate) fn recall_conversation(
    runtime: &ToolRuntime<'_>,
    input: &Value,
) -> Result<Value, String> {
    // Explicit session id/path, else the current session (its dir is the parent
    // of the artifact dir). Text-only transcript: user prompts + final answers,
    // tool calls/results and images stripped (recall_conversation.sh parity).
    let target = match string_input(input, "session") {
        Some(session) => session,
        None => {
            let dir = runtime
                .artifact_dir
                .and_then(|d| d.parent())
                .ok_or("recall_conversation needs a session id/path or an active session")?;
            dir.display().to_string()
        }
    };
    let transcript = crate::recall_conversation_text(&target)?;
    Ok(
        json!({"ok": true, "session": target, "transcript": transcript, "empty": transcript.is_empty()}),
    )
}

/// A question for the operator. The operator ask register answers one the
/// operator already answered in this workspace, in the same words, without
/// asking again; every question asked and its answer go into the register.
pub(crate) fn ask_user(runtime: &ToolRuntime<'_>, input: &Value) -> Result<Value, String> {
    let question = string_input(input, "question").ok_or("ask_user requires question")?;
    let options = input
        .get("options")
        .and_then(Value::as_array)
        .map(|items| {
            items
                .iter()
                .filter_map(Value::as_str)
                .map(ToString::to_string)
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    let session = runtime.artifact_dir.and_then(Path::parent);
    if let Some(session) = session {
        let recalled = crate::completion::recall_question(runtime.cwd, session, &question)?;
        if let Some((ask, answer)) =
            recalled.and_then(|ask| ask.answer.clone().map(|answer| (ask, answer)))
        {
            return Ok(json!({
                "answer": answer.text,
                "fromRegister": {
                    "askId": ask.id,
                    "askedAt": ask.asked_at,
                    "answeredAt": answer.answered_at,
                    "note": "The operator already answered this question; it was not asked again.",
                },
            }));
        }
    }
    let answer = if let Some(ask_user) = runtime.ask_user {
        ask_user(&question, &options)?
    } else {
        if !runtime.interactive {
            return Err("ask_user requires an interactive question channel".into());
        }
        eprintln!("\n[ask_user] {question}");
        if !options.is_empty() {
            for (index, option) in options.iter().enumerate() {
                eprintln!("  {}. {}", index + 1, option);
            }
        }
        eprint!("Answer: ");
        io::stderr().flush().map_err(|e| e.to_string())?;
        let mut answer = String::new();
        let bytes = io::stdin()
            .read_line(&mut answer)
            .map_err(|e| e.to_string())?;
        if bytes == 0 {
            return Err("ask_user requires interactive input".into());
        }
        answer.trim_end_matches(['\r', '\n']).to_string()
    };
    let Some(session) = session else {
        return Ok(json!({"answer": answer}));
    };
    match crate::completion::record_question(runtime.cwd, session, &question, &answer) {
        Ok(id) => Ok(json!({"answer": answer, "askId": id})),
        Err(error) => Ok(json!({"answer": answer, "registerError": error})),
    }
}
