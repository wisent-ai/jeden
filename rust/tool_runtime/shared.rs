use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::fs;
use std::path::{Component, Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use super::ToolRuntime;

pub(crate) const MAX_READ_BYTES: u64 = 512_000;

pub(crate) fn sha256_hex(bytes: &[u8]) -> String {
    hex::encode(Sha256::digest(bytes))
}

pub(crate) fn jail_path(cwd: &Path, input: &str) -> Result<PathBuf, String> {
    let raw = if input.trim().is_empty() { "." } else { input.trim() };
    let path = Path::new(raw);
    if path.is_absolute() {
        return Err(format!("path must be relative to cwd: {raw}"));
    }
    let mut out = cwd.to_path_buf();
    for component in path.components() {
        match component {
            Component::Normal(part) => out.push(part),
            Component::CurDir => {},
            Component::ParentDir => return Err(format!("path escapes cwd: {raw}")),
            _ => return Err(format!("unsupported path component in {raw}")),
        }
    }
    Ok(out)
}

pub(crate) fn string_input(input: &Value, key: &str) -> Option<String> {
    input.get(key).and_then(Value::as_str).map(ToString::to_string)
}

pub(crate) fn bool_input(input: &Value, key: &str, default: bool) -> bool {
    input.get(key).and_then(Value::as_bool).unwrap_or(default)
}

pub(crate) fn u64_input(input: &Value, key: &str, default: u64) -> u64 {
    input.get(key).and_then(Value::as_u64).unwrap_or(default)
}

pub(crate) fn object_input(input: &Value, key: &str) -> Value {
    input.get(key).filter(|value| value.is_object()).cloned().unwrap_or_else(|| json!({}))
}

pub(crate) fn mime_type_for_path(path: &Path) -> &'static str {
    match path.extension().and_then(|ext| ext.to_str()).unwrap_or("").to_ascii_lowercase().as_str() {
        "txt" | "md" | "rs" | "js" | "ts" | "tsx" | "json" | "toml" | "yaml" | "yml" | "html" | "css" | "csv" => "text/plain",
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "gif" => "image/gif",
        "webp" => "image/webp",
        "pdf" => "application/pdf",
        _ => "application/octet-stream",
    }
}

pub(crate) fn parse_line_range_token(token: &str, line_count: usize, full_range: &str) -> Result<(usize, usize), String> {
    let trimmed = token.trim();
    let (start, end) = if let Some((a, b)) = trimmed.split_once('+') {
        let start = a.parse::<usize>().map_err(|_| format!("invalid range: {full_range}"))?;
        let count = b.parse::<usize>().map_err(|_| format!("invalid range: {full_range}"))?;
        (start, start.saturating_add(count).saturating_sub(1))
    } else if let Some((a, b)) = trimmed.split_once('-') {
        let start = a.parse::<usize>().map_err(|_| format!("invalid range: {full_range}"))?;
        let end = if b.is_empty() { line_count } else { b.parse::<usize>().map_err(|_| format!("invalid range: {full_range}"))? };
        (start, end)
    } else {
        let line = trimmed.parse::<usize>().map_err(|_| format!("invalid range: {full_range}"))?;
        (line, line)
    };
    if start == 0 || end < start { return Err(format!("invalid range: {full_range}")); }
    Ok((start, end.min(line_count)))
}

pub(crate) fn line_window(text: &str, range: &str) -> Result<(String, usize, usize, Vec<Value>), String> {
    let lines: Vec<&str> = text.lines().collect();
    if lines.is_empty() { return Ok((String::new(), 1, 0, vec![json!({"startLine": 1, "endLine": 0})])); }
    let mut chunks = Vec::new();
    let mut ranges = Vec::new();
    let mut first_start = usize::MAX;
    let mut last_end = 0usize;
    for token in range.split(',') {
        let (start, end) = parse_line_range_token(token, lines.len(), range)?;
        first_start = first_start.min(start);
        last_end = last_end.max(end);
        let start_idx = start.saturating_sub(1).min(lines.len());
        let end_idx = end.min(lines.len());
        if start_idx < end_idx { chunks.push(lines[start_idx..end_idx].join("\n")); }
        ranges.push(json!({"startLine": start, "endLine": end}));
    }
    Ok((chunks.join("\n"), first_start, last_end, ranges))
}

pub(crate) fn verify_expected_sha(path_label: &str, file: &Path, expected: &str) -> Result<Vec<u8>, String> {
    let bytes = fs::read(file).map_err(|e| e.to_string())?;
    let actual = sha256_hex(&bytes);
    if actual != expected {
        return Err(format!("expectedSha256 mismatch for {path_label}: expected {expected}, actual {actual}"));
    }
    Ok(bytes)
}

pub(crate) fn split_edit_lines(content: &str) -> (Vec<String>, bool) {
    let has_trailing = content.ends_with('\n');
    let body = if has_trailing { &content[..content.len().saturating_sub(1)] } else { content };
    let lines = if body.is_empty() { Vec::new() } else { body.lines().map(ToString::to_string).collect() };
    (lines, has_trailing)
}

pub(crate) fn simple_diff(path: &str, before: &str, after: &str) -> String {
    if before == after { return String::new(); }
    let (old_lines, _) = split_edit_lines(before);
    let (new_lines, _) = split_edit_lines(after);
    let mut out = vec![format!("--- {path}"), format!("+++ {path}"), format!("@@ -1,{} +1,{} @@", old_lines.len(), new_lines.len())];
    for line in old_lines.iter().take(250) { out.push(format!("-{line}")); }
    for line in new_lines.iter().take(250) { out.push(format!("+{line}")); }
    if old_lines.len() + new_lines.len() > 500 { out.push("[diff truncated at 500 lines]".into()); }
    out.join("\n")
}

pub(crate) fn snapshot_tag(hash: &str) -> String {
    hash.chars().take(4).collect::<String>().to_ascii_uppercase()
}

pub(crate) fn snapshot_name(path: &str, hash: &str) -> String {
    format!("{}#{}", path, snapshot_tag(hash))
}

pub(crate) fn run_read_process(runtime: &ToolRuntime<'_>, input: &Value) -> Result<Value, String> {
    let elevated = ToolRuntime {
        cwd: runtime.cwd,
        artifact_dir: runtime.artifact_dir,
        allow_write: runtime.allow_write,
        allow_command: true,
        interactive: runtime.interactive,
    };
    super::exec::run_process(&elevated, input)
}

pub(crate) fn now_millis() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default().as_millis() as u64
}
