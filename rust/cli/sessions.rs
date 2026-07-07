//! Session CLI subcommands: list, show, export, artifacts, resume, recall.

use serde_json::{json, Value};
use std::path::{Path, PathBuf};
use std::fs;

use crate::{agent, read_json, session_root, Args};

pub(crate) fn list_sessions(limit: Option<usize>) -> String {
    let mut rows = vec![];
    if let Ok(entries) = fs::read_dir(session_root()) {
        for entry in entries.flatten().take(limit.unwrap_or(usize::MAX)) { rows.push(entry.file_name().to_string_lossy().to_string()); }
    }
    if rows.is_empty() { "No sessions found.\n".into() } else { rows.join("\n") + "\n" }
}

pub(crate) fn search_sessions_command(args: &Args) -> Result<String, String> {
    let query = args.positionals.first().ok_or("search-sessions requires a query")?.trim().to_ascii_lowercase();
    if query.is_empty() { return Err("search-sessions requires a non-empty query".into()); }
    // Optional positional limit; absent means scan every session (the prior
    // default/clamp were unconsented numeric literals and are dropped).
    let limit = args.positionals.split_first().and_then(|(_, rest)| rest.first()).and_then(|value| value.parse::<usize>().ok());
    let mut rows = Vec::new();
    if let Ok(entries) = fs::read_dir(session_root()) {
        let mut entries = entries.flatten().map(|entry| entry.path()).collect::<Vec<_>>();
        entries.sort_by(|a, b| b.file_name().cmp(&a.file_name()));
        for dir in entries.into_iter().take(limit.unwrap_or(usize::MAX)) {
            let session = match read_session_value(&dir.display().to_string()) {
                Ok(session) => session,
                Err(_) => continue,
            };
            let id = session.get("id").and_then(Value::as_str).unwrap_or("");
            let events = session.get("events").and_then(Value::as_array).cloned().unwrap_or_default();
            for event in events {
                let text = serde_json::to_string(event.get("data").unwrap_or(&Value::Null)).unwrap_or_default();
                let lower = text.to_ascii_lowercase();
                if !lower.contains(&query) { continue; }
                // Whitespace-collapsed full event text (the prior fixed-width
                // char window was an unconsented numeric literal).
                let snippet = text.split_whitespace().collect::<Vec<_>>().join(" ");
                rows.push(format!("{}\t{}\t{}\t{}", id, event.get("ts").and_then(Value::as_str).unwrap_or(""), event.get("type").and_then(Value::as_str).unwrap_or(""), snippet));
                break;
            }
        }
    }
    Ok(if rows.is_empty() { String::new() } else { rows.join("\n") + "\n" })
}

pub(crate) fn session_dir_for(id_or_path: &str) -> PathBuf {
    if id_or_path.contains('/') { PathBuf::from(id_or_path) } else { session_root().join(id_or_path) }
}

/// `jeden resume <id-or-path> ["<task>"]`: load a recorded session's turns into
/// a fresh conversation and, when a task is given, continue it with a real turn
/// (a genuine in-process resume, not just inspection). Accepts `--allow-write`,
/// `--allow-command`, and `--yolo`/`--auto-approve` among the trailing args
/// because the resume parser deliberately skips normal flag handling.
pub(crate) fn resume_command(args: &Args) -> Result<String, String> {
    let (id, rest) = args.positionals.split_first().ok_or("Usage: jeden resume <session-id-or-path> [\"<task>\"]")?;
    let dir = session_dir_for(id);
    if !dir.exists() {
        return Err(format!("session not found: {}", dir.display()));
    }
    let turns = session_conversation_turns(&dir);
    let count = turns.len();
    let mut allow_write = false;
    let mut allow_command = false;
    let mut yolo = false;
    let mut task_parts = Vec::new();
    for part in rest {
        match part.as_str() {
            "--allow-write" => allow_write = true,
            "--allow-command" => allow_command = true,
            "--yolo" | "--auto-approve" => { yolo = true; allow_write = true; allow_command = true; },
            other => task_parts.push(other.to_string()),
        }
    }
    let task = task_parts.join(" ").trim().to_string();
    let mut conversation = agent::Conversation::new(&args.cwd)?;
    conversation.load_history(&args.cwd, turns)?;
    if task.is_empty() {
        return Ok(format!(
            "Loaded {} prior turn(s) from {} into a new session. Continue with: jeden resume {} \"<task>\"\n",
            count, dir.display(), id
        ));
    }
    let mut run_args = args.clone();
    run_args.allow_write = allow_write;
    run_args.allow_command = allow_command;
    run_args.yolo = yolo;
    let mut hooks = agent::RunHooks::inert();
    let text = conversation.run_turn(&run_args, &task, &mut hooks)?;
    let _ = agent::update_last_session_path(&args.cwd, &conversation.session_path());
    Ok(format!("[resumed {} prior turn(s) from {}]\n{}\n", count, dir.display(), text))
}

/// `jeden recall_conversation <id-or-path>`: render a recorded session's full
/// transcript as markdown (recall/inspection).
pub(crate) fn recall_conversation_command(args: &Args) -> Result<String, String> {
    let id = args.positionals.first().ok_or("Usage: jeden recall_conversation <session-id-or-path>")?;
    let value = read_session_value(id)?;
    render_session_export(&value, "markdown")
}

/// Text-only transcript of a recorded session — user prompts and final answers
/// only, with tool calls/results and images stripped. Mirrors the external
/// `recall_conversation.sh` extraction, exposed so the agent `recall_conversation`
/// tool can reload a session's readable history into context.
pub(crate) fn recall_conversation_text(id_or_path: &str) -> Result<String, String> {
    let dir = session_dir_for(id_or_path);
    if !dir.exists() { return Err(format!("session not found: {}", dir.display())); }
    let turns = session_conversation_turns(&dir);
    if turns.is_empty() { return Ok(String::new()); }
    let body = turns.iter().map(|turn| {
        let role = turn.get("role").and_then(Value::as_str).unwrap_or("").to_ascii_uppercase();
        let content = turn.get("content").and_then(Value::as_str).unwrap_or("");
        format!("[{}]\n{}", role, content)
    }).collect::<Vec<_>>().join("\n\n");
    Ok(body)
}

fn read_transcript_events(dir: &Path) -> Vec<Value> {
    let file = dir.join("transcript.jsonl");
    fs::read_to_string(file).unwrap_or_default().lines().filter_map(|line| serde_json::from_str::<Value>(line).ok()).collect()
}

/// Extract prior user/assistant turns from a session transcript so /resume can
/// reload them into the live interactive conversation.
pub(crate) fn session_conversation_turns(dir: &Path) -> Vec<Value> {
    let mut turns = Vec::new();
    for event in read_transcript_events(dir) {
        let kind = event.get("type").and_then(Value::as_str).unwrap_or("");
        let data = event.get("data").cloned().unwrap_or(Value::Null);
        match kind {
            "user" => {
                if let Some(task) = data.get("task").and_then(Value::as_str) {
                    if !task.trim().is_empty() {
                        turns.push(json!({ "role": "user", "content": task }));
                    }
                }
            }
            "final" => {
                if let Some(text) = data.get("text").and_then(Value::as_str) {
                    turns.push(json!({ "role": "assistant", "content": text }));
                }
            }
            _ => {}
        }
    }
    turns
}

pub(crate) fn read_session_value(id_or_path: &str) -> Result<Value, String> {
    let dir = session_dir_for(id_or_path);
    if !dir.exists() { return Err(format!("session not found: {}", dir.display())); }
    let state: Value = read_json(&dir.join("state.json"));
    let id = dir.file_name().map(|v| v.to_string_lossy().to_string()).unwrap_or_else(|| id_or_path.to_string());
    Ok(json!({"id": id, "path": dir, "state": state, "events": read_transcript_events(&dir)}))
}

fn html_escape(value: &str) -> String {
    value.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;").replace('"', "&quot;")
}

pub(crate) fn render_session_export(session: &Value, format: &str) -> Result<String, String> {
    if format == "json" {
        return Ok(serde_json::to_string_pretty(session).map_err(|e| e.to_string())? + "\n");
    }
    let id = session.get("id").and_then(Value::as_str).unwrap_or("session");
    let path = session.get("path").and_then(Value::as_str).unwrap_or("");
    let events = session.get("events").and_then(Value::as_array).cloned().unwrap_or_default();
    if format == "markdown" || format == "md" {
        let mut out = format!("# Jeden session {}\n\n{}\n\n", id, path);
        for event in events {
            let label = format!("{} {}", event.get("ts").and_then(Value::as_str).unwrap_or(""), event.get("type").and_then(Value::as_str).unwrap_or("")).trim().to_string();
            let data = serde_json::to_string_pretty(event.get("data").unwrap_or(&Value::Null)).unwrap_or_else(|_| "{}".into());
            out.push_str(&format!("## {}\n\n```json\n{}\n```\n\n", label, data));
        }
        return Ok(out);
    }
    if format == "html" {
        let mut sections = String::new();
        for event in events {
            let label = html_escape(format!("{} {}", event.get("ts").and_then(Value::as_str).unwrap_or(""), event.get("type").and_then(Value::as_str).unwrap_or("")).trim());
            let body = html_escape(&serde_json::to_string_pretty(event.get("data").unwrap_or(&Value::Null)).unwrap_or_else(|_| "{}".into()));
            sections.push_str(&format!("<section class=\"event\"><h2>{}</h2><pre>{}</pre></section>\n", label, body));
        }
        return Ok(format!("<!doctype html>\n<html><head><meta charset=\"utf-8\"><title>Jeden session {}</title><style>body{{font-family:ui-sans-serif,system-ui,sans-serif;margin:2rem;background:#fafafa;color:#111}}.event{{border:1px solid #ddd;border-radius:8px;background:white;margin:1rem 0;padding:1rem}}pre{{white-space:pre-wrap;overflow-wrap:anywhere}}</style></head><body><h1>Jeden session {}</h1><p>{}</p>{}</body></html>\n", html_escape(id), html_escape(id), html_escape(path), sections));
    }
    Err(format!("unsupported session export format: {}", format))
}

pub(crate) fn export_session_command(args: &Args) -> Result<String, String> {
    let (id, rest) = args.positionals.split_first().ok_or("export requires a session id or path")?;
    let mut format = "json".to_string();
    let mut output = None;
    for arg in rest {
        if arg == "--html" { format = "html".into(); }
        else if arg == "--markdown" { format = "markdown".into(); }
        else { output = Some(arg.clone()); }
    }
    let payload = render_session_export(&read_session_value(id)?, &format)?;
    if let Some(path) = output { fs::write(&path, &payload).map_err(|e| e.to_string())?; Ok(format!("{}\n", path)) } else { Ok(payload) }
}

pub(crate) fn list_artifacts_command(id_or_path: &str) -> Result<String, String> {
    let dir = session_dir_for(id_or_path).join("artifacts");
    let mut rows = vec![];
    if let Ok(entries) = fs::read_dir(dir) {
        for entry in entries.flatten() {
            if let Ok(meta) = entry.metadata() {
                if meta.is_file() { rows.push(format!("{}\t{}", entry.file_name().to_string_lossy(), meta.len())); }
            }
        }
    }
    rows.sort();
    Ok(if rows.is_empty() { String::new() } else { rows.join("\n") + "\n" })
}

pub(crate) fn artifact_command(args: &Args) -> Result<String, String> {
    let mut it = args.positionals.iter();
    let id = it.next().ok_or("artifact requires a session id or path")?;
    let name = it.next().ok_or("artifact requires an artifact name")?;
    let output = it.next();
    let root = session_dir_for(id).join("artifacts");
    let file = root.join(name);
    let canonical_root = fs::canonicalize(&root).map_err(|e| e.to_string())?;
    let canonical_file = fs::canonicalize(&file).map_err(|e| e.to_string())?;
    if !canonical_file.starts_with(&canonical_root) { return Err(format!("artifact path escapes session: {}", name)); }
    let content = fs::read_to_string(&canonical_file).map_err(|e| e.to_string())?;
    if let Some(output) = output { fs::write(output, &content).map_err(|e| e.to_string())?; Ok(format!("{}\n", output)) } else { Ok(if content.ends_with('\n') { content } else { content + "\n" }) }
}
