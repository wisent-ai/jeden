use serde_json::{json, Value};
use std::fs;
use std::io::{self, Write};

use super::shared::{sha256_hex, string_input, u64_input, MAX_READ_BYTES};
use super::ToolRuntime;

mod memory;
mod todo;

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
    if let Some(ask_user) = runtime.ask_user {
        let answer = ask_user(&question, &options)?;
        return Ok(json!({"answer": answer}));
    }
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
    Ok(json!({"answer": answer.trim_end_matches(['\r', '\n']).to_string()}))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    fn runtime_with<'a>(
        ask_user: &'a dyn Fn(&str, &[String]) -> Result<String, String>,
    ) -> ToolRuntime<'a> {
        ToolRuntime {
            cwd: Path::new("."),
            artifact_dir: None,
            operation: crate::tool_runtime::runtime_ops::OperationContext::new(
                crate::tool_runtime::runtime_ops::CancellationToken::new(),
                crate::tool_runtime::runtime_ops::ArtifactSink::new(".jeden/test-artifacts"),
            ),
            allow_write: false,
            allow_command: false,
            interactive: false,
            ask_user: Some(ask_user),
        }
    }

    #[test]
    fn native_ui_ask_user_callback_precedes_noninteractive_refusal_for_options() {
        let callback = |question: &str, options: &[String]| {
            assert_eq!(question, "Which route?");
            assert_eq!(options, ["Alpha", "Beta"]);
            Ok("Beta".into())
        };

        let result = crate::tool_runtime::execute(
            &runtime_with(&callback),
            "ask_user",
            &json!({"question": "Which route?", "options": ["Alpha", "Beta"]}),
        );

        assert_eq!(result, Ok(json!({"answer": "Beta"})));
    }

    #[test]
    fn native_ui_ask_user_callback_preserves_free_text_without_stdin() {
        let callback = |question: &str, options: &[String]| {
            assert_eq!(question, "Describe the change");
            assert!(options.is_empty());
            Ok("  exact free text\r\n".into())
        };

        let result = crate::tool_runtime::execute(
            &runtime_with(&callback),
            "ask_user",
            &json!({"question": "Describe the change"}),
        );

        assert_eq!(result, Ok(json!({"answer": "  exact free text\r\n"})));
    }
}
