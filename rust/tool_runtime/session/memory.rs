use serde_json::{json, Value};
use std::fs;
use std::path::{Path, PathBuf};

use crate::tool_runtime::shared::{now_millis, string_input, u64_input};
use crate::tool_runtime::ToolRuntime;

fn memory_file() -> PathBuf {
    if let Ok(file) = std::env::var("JEDEN_MEMORY_FILE") {
        PathBuf::from(file)
    } else {
        let home = std::env::var("HOME").unwrap_or_else(|_| ".".into());
        Path::new(&home).join(".jeden").join("memory.jsonl")
    }
}

fn memory_scope(scope: Option<&Value>, cwd: &Path) -> Value {
    match scope {
        None | Some(Value::Null) => json!({"kind": "repo", "id": cwd.display().to_string()}),
        Some(Value::String(s)) => {
            if s == "repo" { json!({"kind": "repo", "id": cwd.display().to_string()}) } else { json!({"kind": s, "id": s}) }
        }
        Some(Value::Object(map)) => {
            let kind = map.get("kind").and_then(Value::as_str).unwrap_or("repo");
            let id = map.get("id").and_then(Value::as_str).map(ToString::to_string).unwrap_or_else(|| if kind == "repo" { cwd.display().to_string() } else { kind.to_string() });
            json!({"kind": kind, "id": id})
        }
        Some(other) => json!({"kind": other.to_string(), "id": other.to_string()}),
    }
}

fn memory_load(cwd: &Path) -> Result<Vec<Value>, String> {
    let file = memory_file();
    let raw = match fs::read_to_string(&file) {
        Ok(raw) => raw,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(e) => return Err(e.to_string()),
    };
    let mut records = Vec::new();
    for line in raw.lines().filter(|line| !line.trim().is_empty()) {
        let mut record: Value = serde_json::from_str(line).map_err(|e| e.to_string())?;
        if !(record.get("scope").is_some() && record.get("kind").is_some() && record.get("source").is_some()) {
            record = json!({
                "id": record.get("id").and_then(Value::as_str).map(ToString::to_string).unwrap_or_else(|| format!("m{}", now_millis())),
                "kind": "note",
                "scope": {"kind": "global", "id": "global"},
                "text": record.get("text").and_then(Value::as_str).unwrap_or("").to_string(),
                "tags": record.get("tags").cloned().unwrap_or_else(|| json!([])),
                "source": {"origin": "legacy_memory_tool"},
                "confidence": 0.4,
                "status": "active",
                "createdAt": record.get("createdAt").and_then(Value::as_str).map(ToString::to_string).unwrap_or_else(|| now_millis().to_string()),
            });
        } else if record.get("scope").is_none() {
            record["scope"] = memory_scope(None, cwd);
        }
        records.push(record);
    }
    Ok(records)
}

fn memory_save(records: &[Value]) -> Result<(), String> {
    let file = memory_file();
    if let Some(parent) = file.parent() { fs::create_dir_all(parent).map_err(|e| e.to_string())?; }
    let temp = file.with_extension(format!("{}.tmp", now_millis()));
    let body = records.iter().map(|entry| serde_json::to_string(entry).map_err(|e| e.to_string())).collect::<Result<Vec<_>, _>>()?.join("\n");
    fs::write(&temp, if body.is_empty() { String::new() } else { format!("{body}\n") }).map_err(|e| e.to_string())?;
    fs::rename(temp, file).map_err(|e| e.to_string())
}

fn redact_memory_text(value: &str) -> String {
    let mut text = value.to_string();
    for pattern in [
        r"\b(?:sk|pk|rk)_[A-Za-z0-9_\-]{16,}\b",
        r"\bgh[pousr]_[A-Za-z0-9_]{20,}\b",
        r"\b[A-Za-z0-9+/]{32,}={0,2}\b",
    ] {
        if let Ok(re) = regex::Regex::new(pattern) {
            text = re.replace_all(&text, "[REDACTED]").to_string();
        }
    }
    text
}

fn clip_memory_text(value: &str, max: usize) -> String {
    let text = redact_memory_text(value).split_whitespace().collect::<Vec<_>>().join(" ");
    if text.chars().count() > max {
        let mut clipped = text.chars().take(max.saturating_sub(1)).collect::<String>();
        clipped.push('…');
        clipped
    } else {
        text
    }
}

fn memory_tokens(value: &str) -> Vec<String> {
    value.to_lowercase().split(|ch: char| !(ch.is_alphanumeric() || ch == '_' || ch == '-')).filter(|part| part.len() > 1).map(ToString::to_string).collect()
}

fn memory_score(record: &Value, query: &str) -> f64 {
    let terms = memory_tokens(query);
    let confidence = record.get("confidence").and_then(Value::as_f64).unwrap_or(0.0);
    if terms.is_empty() { return confidence; }
    let text = record.get("text").and_then(Value::as_str).unwrap_or("");
    let kind = record.get("kind").and_then(Value::as_str).unwrap_or("");
    let tags = record.get("tags").and_then(Value::as_array).map(|tags| tags.iter().filter_map(Value::as_str).collect::<Vec<_>>().join(" ")).unwrap_or_default();
    let haystack = memory_tokens(&format!("{text} {tags} {kind}")).join(" ");
    let matches = terms.iter().filter(|term| haystack.contains(term.as_str())).count();
    if matches == 0 { -1.0 } else { matches as f64 / terms.len() as f64 + confidence }
}

fn memory_scope_visible(record_scope: Option<&Value>, requested_scope: &Value) -> bool {
    let Some(scope) = record_scope else { return false; };
    let kind = scope.get("kind").and_then(Value::as_str).unwrap_or("");
    if kind == "global" { return true; }
    if kind != requested_scope.get("kind").and_then(Value::as_str).unwrap_or("") { return false; }
    scope.get("id").and_then(Value::as_str).unwrap_or("") == requested_scope.get("id").and_then(Value::as_str).unwrap_or("")
}

pub(crate) fn memory_tool(runtime: &ToolRuntime<'_>, input: &Value) -> Result<Value, String> {
    let op = string_input(input, "op").unwrap_or_else(|| "recall".into());
    let mut entries = memory_load(runtime.cwd)?;
    if op == "remember" {
        let text = clip_memory_text(&string_input(input, "text").ok_or("memory remember requires text")?, 2_000);
        if text.is_empty() { return Err("memory remember requires text".into()); }
        let tags = input.get("tags").cloned().unwrap_or_else(|| json!([]));
        let entry = json!({
            "id": format!("m{}", now_millis()),
            "kind": string_input(input, "kind").unwrap_or_else(|| "note".into()),
            "scope": memory_scope(input.get("scope"), runtime.cwd),
            "text": text,
            "tags": tags,
            "source": input.get("source").cloned().unwrap_or_else(|| json!({"origin": "rust_memory_tool"})),
            "confidence": input.get("confidence").and_then(Value::as_f64).unwrap_or(0.5).clamp(0.0, 1.0),
            "status": "active",
            "createdAt": now_millis().to_string(),
        });
        entries.push(entry.clone());
        memory_save(&entries)?;
        return Ok(json!({"entry": entry}));
    }
    if op == "list" {
        let limit = u64_input(input, "limit", 20).clamp(1, 200) as usize;
        entries.reverse();
        entries.truncate(limit);
        return Ok(json!({"entries": entries}));
    }
    if op == "recall" {
        let query = string_input(input, "query").unwrap_or_default();
        let requested_scope = memory_scope(input.get("scope"), runtime.cwd);
        let limit = u64_input(input, "limit", 10).clamp(1, 100) as usize;
        let mut scored = Vec::new();
        for entry in entries {
            if entry.get("status").and_then(Value::as_str).is_some_and(|status| status != "active") { continue; }
            if !memory_scope_visible(entry.get("scope"), &requested_scope) { continue; }
            let score = memory_score(&entry, &query);
            if score >= 0.0 { scored.push((score, entry)); }
        }
        scored.sort_by(|a, b| b.0.partial_cmp(&a.0).unwrap_or(std::cmp::Ordering::Equal).then_with(|| b.1.get("createdAt").and_then(Value::as_str).unwrap_or("").cmp(a.1.get("createdAt").and_then(Value::as_str).unwrap_or(""))));
        let matched = scored.into_iter().take(limit).map(|(_, entry)| entry).collect::<Vec<_>>();
        return Ok(json!({"entries": matched, "query": if query.is_empty() { Value::Null } else { json!(query) }}));
    }
    if op == "forget" {
        let before = entries.len();
        if let Some(scope) = input.get("scope") {
            let requested_scope = memory_scope(Some(scope), runtime.cwd);
            entries.retain(|entry| {
                let Some(scope) = entry.get("scope") else { return true; };
                scope.get("kind").and_then(Value::as_str) != requested_scope.get("kind").and_then(Value::as_str) || scope.get("id").and_then(Value::as_str) != requested_scope.get("id").and_then(Value::as_str)
            });
        } else {
            let query = string_input(input, "query").ok_or("memory forget requires scope or query")?;
            entries.retain(|entry| memory_score(entry, &query) < 0.0);
        }
        let removed = before.saturating_sub(entries.len());
        memory_save(&entries)?;
        return Ok(json!({"removed": removed}));
    }
    Err(format!("unknown memory op: {op}"))
}
