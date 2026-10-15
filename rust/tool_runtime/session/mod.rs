use serde_json::{json, Value};
use std::fs;
use std::io::{self, Write};
use std::path::Path;

use super::shared::{count_input, sha256_hex, string_input};
use super::ToolRuntime;

mod context;
mod memory;
mod todo;

pub(crate) use context::context_tool;
pub(crate) use memory::memory_tool;
pub(crate) use todo::todo_tool;

/// The `field` a JSON state file declares as a string. No file declares
/// nothing; one that cannot be read or parsed is refused naming it, so an
/// artifact is never filed under no roadmap item because its state was
/// unreadable.
fn declared_string(path: &Path, field: &str) -> Result<Option<String>, String> {
    let state = crate::slash::common::read_json_value(path)?;
    Ok(state.get(field).and_then(Value::as_str).map(str::to_string))
}

/// The roadmap item an artifact belongs to: the session's own
/// `roadmap-item.json`, else the workspace's active roadmap item.
fn artifact_roadmap_item(runtime: &ToolRuntime<'_>, dir: &Path) -> Result<Option<String>, String> {
    if let Some(session) = dir.parent() {
        if let Some(item) = declared_string(&session.join("roadmap-item.json"), "itemId")? {
            return Ok(Some(item));
        }
    }
    declared_string(
        &runtime.cwd.join(".jeden/mode-state.json"),
        "activeRoadmapItem",
    )
}
pub(crate) fn save_artifact(runtime: &ToolRuntime<'_>, input: &Value) -> Result<Value, String> {
    let Some(dir) = runtime.artifact_dir else {
        return Err("save_artifact requires an active session artifact directory".into());
    };
    let name = string_input(input, "name").ok_or("save_artifact requires name")?;
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
        "roadmapItem": artifact_roadmap_item(runtime, dir)?
    }))
}

pub(crate) fn list_artifacts(runtime: &ToolRuntime<'_>) -> Result<Value, String> {
    let Some(dir) = runtime.artifact_dir else {
        return Err("list_artifacts requires an active session artifact directory".into());
    };
    // No directory yet is no artifacts; one that cannot be listed, or an
    // entry that cannot be read, is refused rather than answered as empty.
    let entries = match fs::read_dir(dir) {
        Ok(entries) => Some(entries),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
        Err(error) => {
            return Err(format!(
                "cannot list artifacts in {}: {error}",
                dir.display()
            ))
        }
    };
    let mut artifacts = Vec::new();
    for entry in entries.into_iter().flatten() {
        let entry = entry
            .map_err(|error| format!("cannot list artifacts in {}: {error}", dir.display()))?;
        let meta = entry.metadata().map_err(|e| e.to_string())?;
        if meta.is_file() {
            artifacts
                .push(json!({"name": entry.file_name().to_string_lossy(), "bytes": meta.len()}));
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
    let max_bytes = count_input(input, "maxBytes", "read_artifact")? as usize;
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
    // of the artifact dir).
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
    // The chat memory of the session's chat answers zoom(id, n) into a line
    // of its view, down to a message whole, and date(id) of a message.
    let memory = || crate::chat_memory::chat::resolve(&target);
    // A message id or a count, given as a number or as its digits.
    let number = |key: &str| match input.get(key) {
        Some(Value::Number(value)) => Some(value.to_string()),
        _ => string_input(input, key),
    };
    if let Some(id) = number("date") {
        return crate::chat_memory::session::date(&memory()?, &id);
    }
    match (number("id"), number("n")) {
        (Some(id), Some(n)) => return crate::chat_memory::session::zoom(&memory()?, &id, &n),
        (Some(_), None) | (None, Some(_)) => {
            return Err(
                "recall_conversation opens a line of the chat memory with both id and n, as the view names it `id+n|`".into(),
            )
        }
        (None, None) => {}
    }
    // Text-only transcript: user prompts + final answers, tool calls/results
    // and images stripped (recall_conversation.sh parity).
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
    // A question asked without options offers none.
    let options: Vec<String> = match input.get("options") {
        None | Some(Value::Null) => Vec::new(),
        Some(Value::Array(items)) => items
            .iter()
            .map(|item| match item {
                Value::String(text) => Ok(text.clone()),
                other => Err(format!(
                    "ask_user options are answers in text; {other} is not"
                )),
            })
            .collect::<Result<_, _>>()?,
        Some(other) => {
            return Err(format!(
                "ask_user options are a list of answers; {other} is not"
            ))
        }
    };
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
            for (number, option) in (std::num::NonZeroUsize::MIN.get()..).zip(&options) {
                eprintln!("  {number}. {option}");
            }
        }
        eprint!("Answer: ");
        io::stderr().flush().map_err(|e| e.to_string())?;
        let mut answer = String::new();
        io::stdin()
            .read_line(&mut answer)
            .map_err(|e| e.to_string())?;
        // Standard input that ended before a line answers nothing.
        if answer.is_empty() {
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
