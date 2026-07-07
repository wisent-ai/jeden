use serde_json::{json, Value};
use std::path::{Path, PathBuf};
use std::{env, fs};

use crate::slash::common::{dirs_home, now_text, split_args, write_json_value};
use crate::slash::SlashContext;

pub(crate) fn memory_file_path() -> PathBuf {
    env::var_os("JEDEN_MEMORY_FILE").map(PathBuf::from).unwrap_or_else(|| dirs_home().join(".jeden/memory.jsonl"))
}

fn memory_artifact_dir(cwd: &Path) -> PathBuf {
    cwd.join(".jeden")
}

fn memory_summary_path(cwd: &Path) -> PathBuf {
    env::var_os("JEDEN_MEMORY_SUMMARY_FILE").map(PathBuf::from).unwrap_or_else(|| memory_artifact_dir(cwd).join("memory_summary.md"))
}

fn memory_document_path(cwd: &Path) -> PathBuf {
    env::var_os("JEDEN_MEMORY_DOCUMENT_FILE").map(PathBuf::from).unwrap_or_else(|| memory_artifact_dir(cwd).join("MEMORY.md"))
}

fn memory_queue_path(cwd: &Path) -> PathBuf {
    memory_artifact_dir(cwd).join("memory-queue.json")
}

fn load_memory_lines() -> Result<Vec<Value>, String> {
    let file = memory_file_path();
    let raw = match fs::read_to_string(&file) {
        Ok(raw) => raw,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(e) => return Err(e.to_string()),
    };
    raw.lines()
        .filter(|line| !line.trim().is_empty())
        .map(|line| serde_json::from_str::<Value>(line).map_err(|e| e.to_string()))
        .collect()
}

fn save_memory_lines(records: &[Value]) -> Result<(), String> {
    let file = memory_file_path();
    if let Some(parent) = file.parent() { fs::create_dir_all(parent).map_err(|e| e.to_string())?; }
    let body = records.iter().map(|record| serde_json::to_string(record).map_err(|e| e.to_string())).collect::<Result<Vec<_>, _>>()?.join("\n");
    fs::write(&file, if body.is_empty() { String::new() } else { format!("{body}\n") }).map_err(|e| e.to_string())
}

fn memory_record_text(record: &Value) -> String {
    record
        .get("text")
        .and_then(Value::as_str)
        .or_else(|| record.get("content").and_then(Value::as_str))
        .unwrap_or("")
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

fn rebuild_memory_artifacts(cwd: &Path, records: &[Value]) -> Result<(PathBuf, PathBuf), String> {
    let summary_path = memory_summary_path(cwd);
    let document_path = memory_document_path(cwd);
    if let Some(parent) = summary_path.parent() { fs::create_dir_all(parent).map_err(|e| e.to_string())?; }
    if let Some(parent) = document_path.parent() { fs::create_dir_all(parent).map_err(|e| e.to_string())?; }

    // The prior "last-N records" cap was an unconsented numeric literal; the
    // summary now carries every record in chronological order.
    let mut bullets = Vec::new();
    for record in records {
        let id = record.get("id").and_then(Value::as_str).unwrap_or("-");
        let kind = record.get("kind").and_then(Value::as_str).unwrap_or("memory");
        let text = memory_record_text(record);
        if !text.is_empty() {
            bullets.push(format!("- [{kind}:{id}] {text}"));
        }
    }
    let summary = if bullets.is_empty() {
        "No durable memory records.".to_string()
    } else {
        format!("Memory Guidance\n\n{}", bullets.join("\n"))
    };
    fs::write(&summary_path, format!("{summary}\n")).map_err(|e| e.to_string())?;

    let mut doc = String::from("# Jeden Memory\n\n");
    doc.push_str(&format!("Records: {}\n\n", records.len()));
    if bullets.is_empty() {
        doc.push_str("No durable memory records.\n");
    } else {
        for record in records {
            let id = record.get("id").and_then(Value::as_str).unwrap_or("-");
            let kind = record.get("kind").and_then(Value::as_str).unwrap_or("memory");
            let text = memory_record_text(record);
            if !text.is_empty() {
                doc.push_str(&format!("- [{kind}:{id}] {text}\n"));
            }
        }
    }
    fs::write(&document_path, doc).map_err(|e| e.to_string())?;
    Ok((summary_path, document_path))
}

pub(crate) fn handle_memory(args: &str, context: &SlashContext<'_>) -> Result<String, String> {
    let argv = split_args(args);
    let (verb, rest) = match argv.split_first() {
        Some((first, rest)) => (first.as_str(), rest),
        None => ("view", &[][..]),
    };
    let records = load_memory_lines()?;
    let file = memory_file_path();
    if matches!(verb, "stats" | "diagnose") {
        let summary = memory_summary_path(context.cwd);
        let queue = memory_queue_path(context.cwd);
        return Ok(format!(
            "Memory file: {}\nRecords: {}\nScope: {}\nSummary: {} ({})\nQueue: {} ({})",
            file.display(),
            records.len(),
            context.cwd.display(),
            summary.display(),
            if summary.exists() { "present" } else { "missing" },
            queue.display(),
            if queue.exists() { "pending" } else { "empty" },
        ));
    }
    if matches!(verb, "clear" | "reset") {
        save_memory_lines(&[])?;
        let _ = fs::remove_file(memory_summary_path(context.cwd));
        let _ = fs::remove_file(memory_document_path(context.cwd));
        let _ = fs::remove_file(memory_queue_path(context.cwd));
        return Ok(format!("Cleared memory backend: {}", file.display()));
    }
    if matches!(verb, "enqueue" | "queue") {
        let queue = memory_queue_path(context.cwd);
        if let Some(parent) = queue.parent() { fs::create_dir_all(parent).map_err(|e| e.to_string())?; }
        write_json_value(&queue, &json!({
            "backend": "local-jsonl",
            "requestedAt": now_text(),
            "cwd": context.cwd,
            "memoryFile": file,
            "records": records.len(),
        }))?;
        return Ok(format!("Memory rebuild enqueued: {}", queue.display()));
    }
    if verb == "rebuild" {
        let (summary, document) = rebuild_memory_artifacts(context.cwd, &records)?;
        let _ = fs::remove_file(memory_queue_path(context.cwd));
        return Ok(format!("Memory rebuilt.\nSummary: {}\nDocument: {}\nRecords: {}", summary.display(), document.display(), records.len()));
    }
    if matches!(verb, "view" | "list" | "") {
        if records.is_empty() { return Ok("No memory records.".into()); }
        let query = rest.join(" ").to_ascii_lowercase();
        let mut shown = records;
        if !query.is_empty() {
            shown.retain(|record| record.get("text").and_then(Value::as_str).unwrap_or("").to_ascii_lowercase().contains(&query));
        }
        // The prior "last-N records" display cap was an unconsented numeric
        // literal; the view now lists every matching record in order.
        return Ok(shown.iter().map(|record| {
            let id = record.get("id").and_then(Value::as_str).unwrap_or("-");
            let kind = record.get("kind").and_then(Value::as_str).unwrap_or("-");
            let text = record.get("text").and_then(Value::as_str).or_else(|| record.get("content").and_then(Value::as_str)).unwrap_or("");
            format!("{id}\t{kind}\t{text}")
        }).collect::<Vec<_>>().join("\n"));
    }
    Err("Usage: /memory [view [query]|stats|diagnose|enqueue|rebuild|clear|reset]".into())
}
