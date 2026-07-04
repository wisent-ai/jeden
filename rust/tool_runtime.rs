use base64::{engine::general_purpose, Engine as _};
use glob::Pattern;
use rusqlite::{types::ValueRef as SqlValueRef, Connection};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::fs;
use std::io::Write;
use std::path::{Component, Path, PathBuf};
use std::process::{Command, Stdio};
use std::thread::sleep;
use std::time::{Duration, Instant};

const MAX_READ_BYTES: u64 = 512_000;
const MAX_SEARCH_FILES: usize = 2_000;

#[derive(Debug, Clone)]
pub struct ToolRuntime<'a> {
    pub cwd: &'a Path,
    pub artifact_dir: Option<&'a Path>,
    pub allow_write: bool,
    pub allow_command: bool,
}

fn sha256_hex(bytes: &[u8]) -> String {
    hex::encode(Sha256::digest(bytes))
}



fn jail_path(cwd: &Path, input: &str) -> Result<PathBuf, String> {
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

fn string_input(input: &Value, key: &str) -> Option<String> {
    input.get(key).and_then(Value::as_str).map(ToString::to_string)
}

fn bool_input(input: &Value, key: &str, default: bool) -> bool {
    input.get(key).and_then(Value::as_bool).unwrap_or(default)
}

fn u64_input(input: &Value, key: &str, default: u64) -> u64 {
    input.get(key).and_then(Value::as_u64).unwrap_or(default)
}
fn object_input(input: &Value, key: &str) -> Value {
    input.get(key).filter(|value| value.is_object()).cloned().unwrap_or_else(|| json!({}))
}


fn mime_type_for_path(path: &Path) -> &'static str {
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

fn parse_line_range_token(token: &str, line_count: usize, full_range: &str) -> Result<(usize, usize), String> {
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

fn line_window(text: &str, range: &str) -> Result<(String, usize, usize, Vec<Value>), String> {
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


fn list_dir(runtime: &ToolRuntime<'_>, input: &Value) -> Result<Value, String> {
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

fn read_file(runtime: &ToolRuntime<'_>, input: &Value) -> Result<Value, String> {
    let path = string_input(input, "path").ok_or("read_file requires path")?;
    let file = jail_path(runtime.cwd, &path)?;
    let meta = fs::metadata(&file).map_err(|e| e.to_string())?;
    if !meta.is_file() { return Err(format!("not a file: {path}")); }
    if meta.len() > MAX_READ_BYTES { return Err(format!("file too large: {} bytes", meta.len())); }
    let bytes = fs::read(&file).map_err(|e| e.to_string())?;
    let content = String::from_utf8(bytes.clone()).map_err(|_| format!("file is not UTF-8: {path}"))?;
    Ok(json!({"ok": true, "path": path, "bytes": bytes.len(), "sha256": sha256_hex(&bytes), "content": content}))
}

fn read_binary_file(runtime: &ToolRuntime<'_>, input: &Value) -> Result<Value, String> {
    let path = string_input(input, "path").ok_or("read_binary_file requires path")?;
    let max_bytes = u64_input(input, "maxBytes", MAX_READ_BYTES).min(MAX_READ_BYTES) as usize;
    let file = jail_path(runtime.cwd, &path)?;
    let meta = fs::metadata(&file).map_err(|e| e.to_string())?;
    if !meta.is_file() { return Err(format!("not a file: {path}")); }
    let bytes = fs::read(&file).map_err(|e| e.to_string())?;
    let truncated = bytes.len() > max_bytes;
    let slice = &bytes[..bytes.len().min(max_bytes)];
    Ok(json!({"ok": true, "path": path, "bytes": bytes.len(), "truncated": truncated, "mimeType": mime_type_for_path(&file), "sha256": sha256_hex(&bytes), "base64": general_purpose::STANDARD.encode(slice)}))
}

fn image_dimensions(bytes: &[u8]) -> Option<(u32, u32, &'static str)> {
    if bytes.len() >= 24 && &bytes[0..8] == b"\x89PNG\r\n\x1a\n" {
        let width = u32::from_be_bytes([bytes[16], bytes[17], bytes[18], bytes[19]]);
        let height = u32::from_be_bytes([bytes[20], bytes[21], bytes[22], bytes[23]]);
        return Some((width, height, "image/png"));
    }
    if bytes.len() >= 10 && &bytes[0..3] == b"GIF" {
        let width = u16::from_le_bytes([bytes[6], bytes[7]]) as u32;
        let height = u16::from_le_bytes([bytes[8], bytes[9]]) as u32;
        return Some((width, height, "image/gif"));
    }
    if bytes.len() >= 12 && &bytes[0..4] == b"RIFF" && &bytes[8..12] == b"WEBP" {
        return Some((0, 0, "image/webp"));
    }
    if bytes.len() > 4 && bytes[0] == 0xff && bytes[1] == 0xd8 {
        let mut i = 2usize;
        while i + 9 < bytes.len() {
            if bytes[i] != 0xff { i += 1; continue; }
            let marker = bytes[i + 1];
            let len = u16::from_be_bytes([bytes[i + 2], bytes[i + 3]]) as usize;
            if matches!(marker, 0xc0 | 0xc1 | 0xc2 | 0xc3) && i + 8 < bytes.len() {
                let height = u16::from_be_bytes([bytes[i + 5], bytes[i + 6]]) as u32;
                let width = u16::from_be_bytes([bytes[i + 7], bytes[i + 8]]) as u32;
                return Some((width, height, "image/jpeg"));
            }
            if len < 2 { break; }
            i += 2 + len;
        }
        return Some((0, 0, "image/jpeg"));
    }
    None
}

fn read_image(runtime: &ToolRuntime<'_>, input: &Value) -> Result<Value, String> {
    let path = string_input(input, "path").ok_or("read_image requires path")?;
    let max_bytes = u64_input(input, "maxBytes", MAX_READ_BYTES).min(MAX_READ_BYTES) as usize;
    let file = jail_path(runtime.cwd, &path)?;
    let bytes = fs::read(&file).map_err(|e| e.to_string())?;
    let Some((width, height, mime_type)) = image_dimensions(&bytes) else {
        return Err(format!("unsupported image type: {}", mime_type_for_path(&file)));
    };
    let truncated = bytes.len() > max_bytes;
    let slice = &bytes[..bytes.len().min(max_bytes)];
    Ok(json!({"ok": true, "path": path, "bytes": bytes.len(), "truncated": truncated, "mimeType": mime_type, "width": width, "height": height, "base64": general_purpose::STANDARD.encode(slice), "sha256": sha256_hex(&bytes)}))
}

fn sql_json_value(value: SqlValueRef<'_>) -> Value {
    match value {
        SqlValueRef::Null => Value::Null,
        SqlValueRef::Integer(n) => json!(n),
        SqlValueRef::Real(n) => json!(n),
        SqlValueRef::Text(bytes) => json!(String::from_utf8_lossy(bytes).to_string()),
        SqlValueRef::Blob(bytes) => json!({"base64": general_purpose::STANDARD.encode(bytes), "bytes": bytes.len()}),
    }
}

fn run_sql_rows(conn: &Connection, sql: &str) -> Result<Vec<Value>, String> {
    let mut stmt = conn.prepare(sql).map_err(|e| e.to_string())?;
    let names = stmt.column_names().iter().map(|name| name.to_string()).collect::<Vec<_>>();
    let mut rows = stmt.query([]).map_err(|e| e.to_string())?;
    let mut out = Vec::new();
    while let Some(row) = rows.next().map_err(|e| e.to_string())? {
        let mut object = serde_json::Map::new();
        for (idx, name) in names.iter().enumerate() {
            object.insert(name.clone(), sql_json_value(row.get_ref(idx).map_err(|e| e.to_string())?));
        }
        out.push(Value::Object(object));
    }
    Ok(out)
}

fn sqlite_identifier(name: &str) -> Result<String, String> {
    if name.is_empty() || name.contains('\0') { return Err(format!("invalid SQLite identifier: {name}")); }
    Ok(format!("\"{}\"", name.replace('"', "\"\"")))
}

fn read_sqlite(runtime: &ToolRuntime<'_>, input: &Value) -> Result<Value, String> {
    let path = string_input(input, "path").ok_or("read_sqlite requires path")?;
    let file = jail_path(runtime.cwd, &path)?;
    let limit = u64_input(input, "limit", 20).clamp(1, 100);
    let offset = u64_input(input, "offset", 0);
    let conn = Connection::open_with_flags(&file, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY).map_err(|e| e.to_string())?;
    if let Some(query) = string_input(input, "query") {
        let query = query.trim().trim_end_matches(';').to_string();
        let lower = query.to_lowercase();
        if query.contains(';') || query.contains('\0') || !(lower.starts_with("select") || lower.starts_with("with")) {
            return Err("read_sqlite query must be a single SELECT or WITH statement".into());
        }
        let rows = run_sql_rows(&conn, &format!("SELECT * FROM ({query}) LIMIT {limit} OFFSET {offset}"))?;
        return Ok(json!({"path": path, "query": query, "rows": rows, "limit": limit, "offset": offset}));
    }
    let table = string_input(input, "table");
    if table.is_none() {
        let mut tables = run_sql_rows(&conn, "SELECT name, type FROM sqlite_schema WHERE type IN ('table','view') AND name NOT LIKE 'sqlite_%' ORDER BY name")?;
        for table in &mut tables {
            if table.get("type").and_then(Value::as_str) == Some("table") {
                if let Some(name) = table.get("name").and_then(Value::as_str) {
                    let count_rows = run_sql_rows(&conn, &format!("SELECT count(*) AS count FROM {}", sqlite_identifier(name)?))?;
                    table["rows"] = count_rows.get(0).and_then(|row| row.get("count")).cloned().unwrap_or(Value::Null);
                }
            }
        }
        return Ok(json!({"path": path, "tables": tables}));
    }
    let table_name = table.unwrap();
    let table_sql = sqlite_identifier(&table_name)?;
    let schema = run_sql_rows(&conn, &format!("PRAGMA table_info({table_sql})"))?;
    if let Some(key) = string_input(input, "key") {
        let primary_keys = schema.iter().filter(|column| column.get("pk").and_then(Value::as_i64).unwrap_or(0) > 0).collect::<Vec<_>>();
        if primary_keys.len() != 1 { return Err(format!("table has no single-column primary key: {table_name}")); }
        let pk = primary_keys[0].get("name").and_then(Value::as_str).ok_or("primary key has no name")?;
        let escaped_key = key.replace('\'', "''");
        let rows = run_sql_rows(&conn, &format!("SELECT * FROM {table_sql} WHERE {} = '{escaped_key}' LIMIT 1", sqlite_identifier(pk)?))?;
        return Ok(json!({"path": path, "table": table_name, "schema": schema, "row": rows.into_iter().next()}));
    }
    let mut clauses = Vec::new();
    if let Some(where_clause) = string_input(input, "where") { clauses.push(format!("WHERE {where_clause}")); }
    if let Some(order) = string_input(input, "order") { clauses.push(format!("ORDER BY {order}")); }
    clauses.push(format!("LIMIT {limit}"));
    if offset > 0 { clauses.push(format!("OFFSET {offset}")); }
    let rows = run_sql_rows(&conn, &format!("SELECT * FROM {table_sql} {}", clauses.join(" ")))?;
    Ok(json!({"path": path, "table": table_name, "schema": schema, "rows": rows, "limit": limit, "offset": offset}))
}





fn write_file(runtime: &ToolRuntime<'_>, input: &Value) -> Result<Value, String> {
    if !runtime.allow_write { return Err("write_file requires --allow-write".into()); }
    let path = string_input(input, "path").ok_or("write_file requires path")?;
    let content = string_input(input, "content").ok_or("write_file requires content")?;
    let file = jail_path(runtime.cwd, &path)?;
    if file.exists() {
        let expected = string_input(input, "expectedSha256").ok_or("write_file overwrite requires expectedSha256")?;
        let old = fs::read(&file).map_err(|e| e.to_string())?;
        let actual = sha256_hex(&old);
        if actual != expected { return Err(format!("expectedSha256 mismatch for {path}: expected {expected}, actual {actual}")); }
    }
    if let Some(parent) = file.parent() { fs::create_dir_all(parent).map_err(|e| e.to_string())?; }
    fs::write(&file, content.as_bytes()).map_err(|e| e.to_string())?;
    Ok(json!({"ok": true, "path": path, "bytes": content.len(), "sha256": sha256_hex(content.as_bytes())}))
}

fn verify_expected_sha(path_label: &str, file: &Path, expected: &str) -> Result<Vec<u8>, String> {
    let bytes = fs::read(file).map_err(|e| e.to_string())?;
    let actual = sha256_hex(&bytes);
    if actual != expected {
        return Err(format!("expectedSha256 mismatch for {path_label}: expected {expected}, actual {actual}"));
    }
    Ok(bytes)
}

fn delete_file(runtime: &ToolRuntime<'_>, input: &Value) -> Result<Value, String> {
    if !runtime.allow_write { return Err("delete_file requires --allow-write".into()); }
    let path = string_input(input, "path").ok_or("delete_file requires path")?;
    let expected = string_input(input, "expectedSha256").ok_or("delete_file requires expectedSha256")?;
    let file = jail_path(runtime.cwd, &path)?;
    let bytes = verify_expected_sha(&path, &file, &expected)?;
    fs::remove_file(&file).map_err(|e| e.to_string())?;
    Ok(json!({"ok": true, "path": path, "deleted": true, "previousSha256": sha256_hex(&bytes), "previousBytes": bytes.len()}))
}

fn move_file(runtime: &ToolRuntime<'_>, input: &Value) -> Result<Value, String> {
    if !runtime.allow_write { return Err("move_file requires --allow-write".into()); }
    let from = string_input(input, "from").ok_or("move_file requires from")?;
    let to = string_input(input, "to").ok_or("move_file requires to")?;
    let expected = string_input(input, "expectedSha256").ok_or("move_file requires expectedSha256")?;
    let overwrite = bool_input(input, "overwrite", false);
    let from_file = jail_path(runtime.cwd, &from)?;
    let to_file = jail_path(runtime.cwd, &to)?;
    let bytes = verify_expected_sha(&from, &from_file, &expected)?;
    if to_file.exists() && !overwrite { return Err(format!("destination exists: {to}")); }
    if let Some(parent) = to_file.parent() { fs::create_dir_all(parent).map_err(|e| e.to_string())?; }
    fs::rename(&from_file, &to_file).map_err(|e| e.to_string())?;
    Ok(json!({"ok": true, "from": from, "to": to, "sha256": sha256_hex(&bytes), "bytes": bytes.len()}))
}


fn search_text(runtime: &ToolRuntime<'_>, input: &Value) -> Result<Value, String> {
    let query = string_input(input, "query").ok_or("search_text requires query")?;
    let path = string_input(input, "path").ok_or("search_text requires path")?;
    let case_sensitive = bool_input(input, "caseSensitive", false);
    let file = jail_path(runtime.cwd, &path)?;
    let content = fs::read_to_string(&file).map_err(|e| e.to_string())?;
    let needle = if case_sensitive { query.clone() } else { query.to_lowercase() };
    let mut matches = Vec::new();
    for (idx, line) in content.lines().enumerate() {
        let hay = if case_sensitive { line.to_string() } else { line.to_lowercase() };
        if hay.contains(&needle) {
            matches.push(json!({"line": idx + 1, "text": line}));
            if matches.len() >= 50 { break; }
        }
    }
    Ok(json!({"ok": true, "path": path, "query": query, "matches": matches}))
}

fn rel_path(cwd: &Path, file: &Path) -> String {
    file.strip_prefix(cwd).unwrap_or(file).to_string_lossy().replace('\\', "/")
}

fn skip_discovery_dir(name: &str) -> bool {
    matches!(name, ".git" | "node_modules" | "dist" | "build" | "coverage" | ".next" | "target" | ".turbo" | ".vercel")
}

fn collect_files(root: &Path, hidden: bool, out: &mut Vec<PathBuf>) -> Result<(), String> {
    if out.len() >= MAX_SEARCH_FILES { return Ok(()); }
    let entries = fs::read_dir(root).map_err(|e| e.to_string())?;
    for entry in entries.flatten() {
        if out.len() >= MAX_SEARCH_FILES { break; }
        let path = entry.path();
        let name = entry.file_name().to_string_lossy().to_string();
        if !hidden && name.starts_with('.') { continue; }
        let meta = match entry.metadata() { Ok(meta) => meta, Err(_) => continue };
        if meta.is_dir() {
            if skip_discovery_dir(&name) { continue; }
            collect_files(&path, hidden, out)?;
        } else if meta.is_file() {
            out.push(path);
        }
    }
    Ok(())
}

fn collect_entries(root: &Path, hidden: bool, out: &mut Vec<(PathBuf, &'static str)>) -> Result<(), String> {
    if out.len() >= MAX_SEARCH_FILES { return Ok(()); }
    let entries = fs::read_dir(root).map_err(|e| e.to_string())?;
    for entry in entries.flatten() {
        if out.len() >= MAX_SEARCH_FILES { break; }
        let path = entry.path();
        let name = entry.file_name().to_string_lossy().to_string();
        if !hidden && name.starts_with('.') { continue; }
        let meta = match entry.metadata() { Ok(meta) => meta, Err(_) => continue };
        if meta.is_dir() {
            if skip_discovery_dir(&name) { continue; }
            out.push((path.clone(), "directory"));
            collect_entries(&path, hidden, out)?;
        } else if meta.is_file() {
            out.push((path, "file"));
        }
    }
    Ok(())
}


fn input_roots(runtime: &ToolRuntime<'_>, input: &Value) -> Result<Vec<PathBuf>, String> {
    if let Some(paths) = input.get("paths").and_then(Value::as_array) {
        return paths.iter().filter_map(Value::as_str).map(|path| jail_path(runtime.cwd, path)).collect();
    }
    let path = string_input(input, "path").unwrap_or_else(|| ".".into());
    Ok(vec![jail_path(runtime.cwd, &path)?])
}

fn text_files_for_input(runtime: &ToolRuntime<'_>, input: &Value) -> Result<Vec<PathBuf>, String> {
    let hidden = bool_input(input, "hidden", false);
    let mut files = Vec::new();
    for root in input_roots(runtime, input)? {
        let meta = fs::metadata(&root).map_err(|e| e.to_string())?;
        if meta.is_file() { files.push(root); }
        else if meta.is_dir() { collect_files(&root, hidden, &mut files)?; }
    }
    files.sort();
    Ok(files)
}

fn search_files(runtime: &ToolRuntime<'_>, input: &Value) -> Result<Value, String> {
    let query = string_input(input, "query").ok_or("search_files requires query")?;
    let limit = u64_input(input, "limit", 500).clamp(1, 500) as usize;
    let skip = u64_input(input, "skip", 0) as usize;
    let case_sensitive = bool_input(input, "caseSensitive", false);
    let needle = if case_sensitive { query.clone() } else { query.to_lowercase() };
    let files = text_files_for_input(runtime, input)?;
    let mut seen = 0usize;
    let mut matches = Vec::new();
    for file in &files {
        if matches.len() >= limit { break; }
        let Ok(content) = fs::read_to_string(file) else { continue };
        if content.contains('\0') { continue; }
        for (idx, line) in content.lines().enumerate() {
            let hay = if case_sensitive { line.to_string() } else { line.to_lowercase() };
            if hay.contains(&needle) {
                seen += 1;
                if seen <= skip { continue; }
                matches.push(json!({"path": rel_path(runtime.cwd, file), "line": idx + 1, "text": line}));
                if matches.len() >= limit { break; }
            }
        }
    }
    Ok(json!({"searchedFiles": files.len(), "skip": skip, "limit": limit, "matches": matches}))
}

fn glob_paths(runtime: &ToolRuntime<'_>, input: &Value) -> Result<Value, String> {
    let root = jail_path(runtime.cwd, &string_input(input, "path").unwrap_or_else(|| ".".into()))?;
    let limit = u64_input(input, "limit", 200).clamp(1, 2_000) as usize;
    let skip = u64_input(input, "skip", 0) as usize;
    let hidden = bool_input(input, "hidden", false);
    let raw_patterns = if let Some(patterns) = input.get("patterns").and_then(Value::as_array) {
        patterns.iter().filter_map(Value::as_str).map(ToString::to_string).collect::<Vec<_>>()
    } else {
        vec![string_input(input, "patterns").unwrap_or_else(|| "**".into())]
    };
    let patterns = raw_patterns.iter().filter_map(|pattern| Pattern::new(pattern).ok()).collect::<Vec<_>>();
    let mut paths = Vec::new();
    collect_entries(&root, hidden, &mut paths)?;
    let mut entries = paths.into_iter().filter_map(|(file, kind)| {
        let path = rel_path(runtime.cwd, &file);
        if patterns.iter().any(|pattern| pattern.matches(&path)) { Some(json!({"path": path, "type": kind})) } else { None }
    }).collect::<Vec<_>>();
    entries.sort_by_key(|entry| entry.get("path").and_then(Value::as_str).unwrap_or("").to_string());
    let matches = entries.into_iter().skip(skip).take(limit).collect::<Vec<_>>();
    Ok(json!({"skip": skip, "limit": limit, "matches": matches}))
}

fn grep_regex(runtime: &ToolRuntime<'_>, input: &Value) -> Result<Value, String> {
    let expr = string_input(input, "expr").ok_or("grep_regex requires expr")?;
    let limit = u64_input(input, "limit", 500).clamp(1, 500) as usize;
    let skip = u64_input(input, "skip", 0) as usize;
    let case_sensitive = bool_input(input, "caseSensitive", false);
    let multiline = bool_input(input, "multiline", false) || expr.contains('\n');
    let matcher = regex::RegexBuilder::new(&expr).case_insensitive(!case_sensitive).dot_matches_new_line(multiline).build().map_err(|e| e.to_string())?;
    let files = text_files_for_input(runtime, input)?;
    let mut seen = 0usize;
    let mut matches = Vec::new();
    for file in &files {
        if matches.len() >= limit { break; }
        let Ok(content) = fs::read_to_string(file) else { continue };
        if content.contains('\0') { continue; }
        if multiline {
            for mat in matcher.find_iter(&content) {
                seen += 1;
                if seen <= skip { continue; }
                let line = content[..mat.start()].lines().count() + 1;
                matches.push(json!({"path": rel_path(runtime.cwd, file), "line": line, "text": mat.as_str().split_whitespace().collect::<Vec<_>>().join(" ").chars().take(500).collect::<String>()}));
                if matches.len() >= limit { break; }
            }
        } else {
            for (idx, line) in content.lines().enumerate() {
                if matcher.is_match(line) {
                    seen += 1;
                    if seen <= skip { continue; }
                    matches.push(json!({"path": rel_path(runtime.cwd, file), "line": idx + 1, "text": line}));
                    if matches.len() >= limit { break; }
                }
            }
        }
    }
    Ok(json!({"searchedFiles": files.len(), "skip": skip, "limit": limit, "matches": matches}))
}


fn run_command(runtime: &ToolRuntime<'_>, input: &Value) -> Result<Value, String> {
    if !runtime.allow_command { return Err("run_command requires --allow-command".into()); }
    let command = string_input(input, "command").ok_or("run_command requires command")?;
    let timeout_ms = u64_input(input, "timeoutMs", 30_000).min(120_000);
    let deadline = Duration::from_millis(timeout_ms);
    let mut child = Command::new("sh")
        .arg("-c")
        .arg(&command)
        .current_dir(runtime.cwd)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| e.to_string())?;
    let started = Instant::now();
    loop {
        if child.try_wait().map_err(|e| e.to_string())?.is_some() {
            let output = child.wait_with_output().map_err(|e| e.to_string())?;
            let stdout = String::from_utf8_lossy(&output.stdout).to_string();
            let stderr = String::from_utf8_lossy(&output.stderr).to_string();
            return Ok(json!({"ok": output.status.success(), "command": command, "timeoutMs": timeout_ms, "timedOut": false, "code": output.status.code(), "stdout": stdout, "stderr": stderr}));
        }
        if started.elapsed() >= deadline {
            let _ = child.kill();
            let output = child.wait_with_output().map_err(|e| e.to_string())?;
            let stdout = String::from_utf8_lossy(&output.stdout).to_string();
            let stderr = String::from_utf8_lossy(&output.stderr).to_string();
            return Ok(json!({"ok": false, "command": command, "timeoutMs": timeout_ms, "timedOut": true, "code": output.status.code(), "stdout": stdout, "stderr": stderr}));
        }
        sleep(Duration::from_millis(20));
    }
}

fn run_process(runtime: &ToolRuntime<'_>, input: &Value) -> Result<Value, String> {
    if !runtime.allow_command { return Err("run_process requires --allow-command".into()); }
    let command = string_input(input, "command").ok_or("run_process requires command")?;
    let args = input.get("args").and_then(Value::as_array).map(|values| values.iter().map(|value| value.as_str().map(ToString::to_string).unwrap_or_else(|| value.to_string())).collect::<Vec<_>>()).unwrap_or_default();
    let stdin = string_input(input, "stdin");
    let timeout_ms = u64_input(input, "timeoutMs", 30_000).clamp(1_000, 120_000);
    let deadline = Duration::from_millis(timeout_ms);
    let mut command_builder = Command::new(&command);
    command_builder
        .args(&args)
        .current_dir(runtime.cwd)
        .stdin(if stdin.is_some() { Stdio::piped() } else { Stdio::null() })
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    if let Some(env) = input.get("env").and_then(Value::as_object) {
        for (key, value) in env {
            if value.is_null() {
                command_builder.env_remove(key);
            } else {
                command_builder.env(key, value.as_str().map(ToString::to_string).unwrap_or_else(|| value.to_string()));
            }
        }
    }
    let mut child = command_builder.spawn().map_err(|e| e.to_string())?;
    if let Some(stdin) = stdin {
        if let Some(mut pipe) = child.stdin.take() { pipe.write_all(stdin.as_bytes()).map_err(|e| e.to_string())?; }
    }
    let started = Instant::now();
    loop {
        if child.try_wait().map_err(|e| e.to_string())?.is_some() {
            let output = child.wait_with_output().map_err(|e| e.to_string())?;
            return Ok(json!({"ok": output.status.success(), "command": command, "args": args, "timeoutMs": timeout_ms, "timedOut": false, "code": output.status.code(), "stdout": String::from_utf8_lossy(&output.stdout), "stderr": String::from_utf8_lossy(&output.stderr)}));
        }
        if started.elapsed() >= deadline {
            let _ = child.kill();
            let output = child.wait_with_output().map_err(|e| e.to_string())?;
            return Ok(json!({"ok": false, "command": command, "args": args, "timeoutMs": timeout_ms, "timedOut": true, "code": output.status.code(), "stdout": String::from_utf8_lossy(&output.stdout), "stderr": String::from_utf8_lossy(&output.stderr)}));
        }
        sleep(Duration::from_millis(20));
    }
}

fn node_eval(runtime: &ToolRuntime<'_>, input: &Value) -> Result<Value, String> {
    let code = string_input(input, "code").ok_or("node_eval requires code")?;
    run_process(runtime, &json!({"command": "node", "args": ["--input-type=module", "-"], "stdin": code, "timeoutMs": u64_input(input, "timeoutMs", 30_000)}))
}

fn python_eval(runtime: &ToolRuntime<'_>, input: &Value) -> Result<Value, String> {
    let code = string_input(input, "code").ok_or("python_eval requires code")?;
    run_process(runtime, &json!({"command": "python3", "args": ["-"], "stdin": code, "timeoutMs": u64_input(input, "timeoutMs", 30_000)}))
}


fn list_package_scripts(runtime: &ToolRuntime<'_>) -> Result<Value, String> {
    let file = runtime.cwd.join("package.json");
    let raw = fs::read_to_string(&file).map_err(|e| e.to_string())?;
    let parsed: Value = serde_json::from_str(&raw).map_err(|e| e.to_string())?;
    let mut scripts = serde_json::Map::new();
    if let Some(raw_scripts) = parsed.get("scripts").and_then(Value::as_object) {
        for (name, value) in raw_scripts {
            if let Some(script) = value.as_str() { scripts.insert(name.clone(), json!(script)); }
        }
    }
    Ok(Value::Object(scripts))
}

fn run_package_script(runtime: &ToolRuntime<'_>, input: &Value) -> Result<Value, String> {
    if !runtime.allow_command { return Err("run_package_script requires --allow-command".into()); }
    let script = string_input(input, "script").ok_or("run_package_script requires script")?;
    let scripts = list_package_scripts(runtime)?;
    if scripts.get(&script).and_then(Value::as_str).is_none() {
        return Err(format!("unknown package script: {script}"));
    }
    let mut payload = json!({"command": "npm", "args": ["run", script], "timeoutMs": u64_input(input, "timeoutMs", 60_000).clamp(1_000, 180_000)});
    if let Some(env) = input.get("env") { payload["env"] = env.clone(); }
    run_process(runtime, &payload)
}

fn run_read_process(runtime: &ToolRuntime<'_>, input: &Value) -> Result<Value, String> {
    let elevated = ToolRuntime {
        cwd: runtime.cwd,
        artifact_dir: runtime.artifact_dir,
        allow_write: runtime.allow_write,
        allow_command: true,
    };
    run_process(&elevated, input)
}


fn git_status(runtime: &ToolRuntime<'_>) -> Result<Value, String> {
    run_read_process(runtime, &json!({"command": "git", "args": ["status", "--short"], "timeoutMs": 30_000}))
}

fn git_diff(runtime: &ToolRuntime<'_>, input: &Value) -> Result<Value, String> {
    let mut args = vec!["diff".to_string(), "--".to_string()];
    if let Some(path) = string_input(input, "path") {
        let _ = jail_path(runtime.cwd, &path)?;
        args.push(path);
    }
    run_read_process(runtime, &json!({"command": "git", "args": args, "timeoutMs": 30_000}))
}

fn git_log(runtime: &ToolRuntime<'_>, input: &Value) -> Result<Value, String> {
    let limit = u64_input(input, "limit", 20).clamp(1, 100);
    let mut args = vec!["log".to_string(), format!("-{limit}"), "--oneline".to_string(), "--decorate".to_string(), "--".to_string()];
    if let Some(path) = string_input(input, "path") {
        let _ = jail_path(runtime.cwd, &path)?;
        args.push(path);
    }
    run_read_process(runtime, &json!({"command": "git", "args": args, "timeoutMs": 30_000}))
}

fn git_show(runtime: &ToolRuntime<'_>, input: &Value) -> Result<Value, String> {
    let reference = string_input(input, "ref").unwrap_or_else(|| "HEAD".into());
    let mut args = vec!["show".to_string(), "--stat".to_string(), "--oneline".to_string(), "--decorate".to_string(), reference, "--".to_string()];
    if let Some(path) = string_input(input, "path") {
        let _ = jail_path(runtime.cwd, &path)?;
        args.push(path);
    }
    run_read_process(runtime, &json!({"command": "git", "args": args, "timeoutMs": 30_000}))
}


fn fetch_url(_runtime: &ToolRuntime<'_>, input: &Value) -> Result<Value, String> {
    let url = string_input(input, "url").ok_or("fetch_url requires url")?;
    if !url.starts_with("http://") && !url.starts_with("https://") { return Err("fetch_url requires http(s) URL".into()); }
    let timeout_ms = u64_input(input, "timeoutMs", 30_000).clamp(1_000, 120_000);
    let max_bytes = u64_input(input, "maxBytes", 200_000).clamp(1_000, 1_000_000) as usize;
    let client = reqwest::blocking::Client::builder().timeout(Duration::from_millis(timeout_ms)).build().map_err(|e| e.to_string())?;
    let response = client.get(&url).send().map_err(|e| e.to_string())?;
    let status = response.status().as_u16();
    let content_type = response.headers().get(reqwest::header::CONTENT_TYPE).and_then(|value| value.to_str().ok()).map(ToString::to_string);
    let bytes = response.bytes().map_err(|e| e.to_string())?;
    let raw_text = String::from_utf8_lossy(&bytes).to_string();
    let (selected_text, start_line, end_line, ranges) = if let Some(range) = string_input(input, "range") {
        line_window(&raw_text, &range)?
    } else {
        (raw_text, 0, 0, Vec::new())
    };
    let output_bytes = selected_text.as_bytes();
    let truncated = output_bytes.len() > max_bytes;
    let slice = &output_bytes[..output_bytes.len().min(max_bytes)];
    let text = String::from_utf8_lossy(slice).to_string();
    Ok(json!({"ok": status >= 200 && status < 300, "url": url, "status": status, "contentType": content_type, "bytes": bytes.len(), "truncated": truncated, "sha256": sha256_hex(&bytes), "text": text, "startLine": if ranges.is_empty() { Value::Null } else { json!(start_line) }, "endLine": if ranges.is_empty() { Value::Null } else { json!(end_line) }, "ranges": if ranges.is_empty() { Value::Null } else { json!(ranges) }}))
}




fn save_artifact(runtime: &ToolRuntime<'_>, input: &Value) -> Result<Value, String> {
    let Some(dir) = runtime.artifact_dir else { return Err("save_artifact requires an active session artifact directory".into()); };
    let name = string_input(input, "name").unwrap_or_else(|| "artifact.txt".into());
    let content = string_input(input, "content").ok_or("save_artifact requires content")?;
    if name.contains('/') || name.contains("..") { return Err(format!("invalid artifact name: {name}")); }
    fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    let path = dir.join(&name);
    fs::write(&path, content.as_bytes()).map_err(|e| e.to_string())?;
    Ok(json!({"ok": true, "name": name, "path": path.display().to_string(), "bytes": content.len()}))
}

fn list_artifacts(runtime: &ToolRuntime<'_>) -> Result<Value, String> {
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

fn read_artifact(runtime: &ToolRuntime<'_>, input: &Value) -> Result<Value, String> {
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

fn todo_item(value: &Value) -> Value {
    let text = value.as_str()
        .or_else(|| value.get("text").and_then(Value::as_str))
        .or_else(|| value.get("task").and_then(Value::as_str))
        .or_else(|| value.get("name").and_then(Value::as_str))
        .unwrap_or("");
    let status = value.get("status").and_then(Value::as_str).unwrap_or("pending");
    json!({"text": text, "status": status})
}

fn todo_summary(state: &mut Value) -> Value {
    let mut items_flat = Vec::new();
    let mut has_active = false;
    if let Some(phases) = state.get_mut("phases").and_then(Value::as_array_mut) {
        for phase in phases.iter_mut() {
            if let Some(items) = phase.get_mut("items").and_then(Value::as_array_mut) {
                if items.iter().any(|item| item.get("status").and_then(Value::as_str) == Some("in_progress")) {
                    has_active = true;
                }
            }
        }
        if !has_active {
            'outer: for phase in phases.iter_mut() {
                if let Some(items) = phase.get_mut("items").and_then(Value::as_array_mut) {
                    for item in items {
                        if item.get("status").and_then(Value::as_str) == Some("pending") {
                            item["status"] = json!("in_progress");
                            break 'outer;
                        }
                    }
                }
            }
        }
        for phase in phases.iter() {
            let phase_name = phase.get("phase").and_then(Value::as_str).unwrap_or("Tasks");
            if let Some(items) = phase.get("items").and_then(Value::as_array) {
                for item in items {
                    let mut flat = item.clone();
                    flat["phase"] = json!(phase_name);
                    items_flat.push(flat);
                }
            }
        }
    }
    let completed = items_flat.iter().filter(|item| item.get("status").and_then(Value::as_str) == Some("done")).count();
    let active = items_flat.iter()
        .find(|item| item.get("status").and_then(Value::as_str) == Some("in_progress"))
        .or_else(|| items_flat.iter().find(|item| !matches!(item.get("status").and_then(Value::as_str), Some("done" | "dropped"))))
        .and_then(|item| item.get("text").and_then(Value::as_str))
        .map(ToString::to_string);
    json!({"total": items_flat.len(), "completed": completed, "active": active, "phases": state.get("phases").cloned().unwrap_or_else(|| json!([])), "items": items_flat})
}

fn todo_tool(runtime: &ToolRuntime<'_>, input: &Value) -> Result<Value, String> {
    let Some(dir) = runtime.artifact_dir else { return Err("todo requires an active session".into()); };
    fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    let file = dir.join("todo.json");
    let mut state: Value = fs::read_to_string(&file).ok().and_then(|raw| serde_json::from_str(&raw).ok()).unwrap_or_else(|| json!({"phases": []}));
    let op = string_input(input, "op").unwrap_or_else(|| "view".into());
    if op == "init" {
        if let Some(list) = input.get("list").and_then(Value::as_array) {
            let phases = list.iter().map(|phase| {
                let name = phase.get("phase").or_else(|| phase.get("name")).and_then(Value::as_str).unwrap_or("Tasks");
                let items = phase.get("items").and_then(Value::as_array).cloned().unwrap_or_default().iter().map(todo_item).collect::<Vec<_>>();
                json!({"phase": name, "items": items})
            }).collect::<Vec<_>>();
            state["phases"] = json!(phases);
        } else {
            let name = string_input(input, "phase").unwrap_or_else(|| "Tasks".into());
            let items = input.get("items").and_then(Value::as_array).cloned().unwrap_or_default().iter().map(todo_item).collect::<Vec<_>>();
            state["phases"] = json!([{"phase": name, "items": items}]);
        }
    } else if op == "append" {
        let name = string_input(input, "phase").unwrap_or_else(|| "Tasks".into());
        let new_items = input.get("items").and_then(Value::as_array).ok_or("items are required")?;
        let phases = state["phases"].as_array_mut().ok_or("invalid todo state")?;
        let idx = phases.iter().position(|phase| phase.get("phase").and_then(Value::as_str) == Some(name.as_str())).unwrap_or_else(|| {
            phases.push(json!({"phase": name, "items": []}));
            phases.len() - 1
        });
        let items = phases[idx]["items"].as_array_mut().ok_or("invalid todo phase")?;
        items.extend(new_items.iter().map(todo_item));
    } else if matches!(op.as_str(), "start" | "done" | "drop") {
        let status = if op == "start" { "in_progress" } else if op == "done" { "done" } else { "dropped" };
        let mut found = false;
        if let Some(phase_name) = string_input(input, "phase").filter(|_| op != "start" && string_input(input, "task").is_none()) {
            if let Some(phases) = state["phases"].as_array_mut() {
                for phase in phases {
                    if phase.get("phase").and_then(Value::as_str) == Some(phase_name.as_str()) {
                        if let Some(items) = phase["items"].as_array_mut() {
                            for item in items { item["status"] = json!(status); }
                            found = true;
                        }
                    }
                }
            }
            if !found { return Err(format!("unknown phase: {phase_name}")); }
        } else {
            let task = string_input(input, "task").ok_or("task is required")?;
            if let Some(phases) = state["phases"].as_array_mut() {
                for phase in phases {
                    if let Some(items) = phase["items"].as_array_mut() {
                        for item in items {
                            if op == "start" && item.get("status").and_then(Value::as_str) == Some("in_progress") { item["status"] = json!("pending"); }
                            if item.get("text").and_then(Value::as_str) == Some(task.as_str()) { item["status"] = json!(status); found = true; }
                        }
                    }
                }
            }
            if !found { return Err(format!("unknown task: {task}")); }
        }
    } else if op == "rm" {
        if let Some(phase_name) = string_input(input, "phase") {
            if let Some(phases) = state["phases"].as_array_mut() {
                phases.retain(|phase| phase.get("phase").and_then(Value::as_str) != Some(phase_name.as_str()));
            }
        } else if let Some(task) = string_input(input, "task") {
            if let Some(phases) = state["phases"].as_array_mut() {
                for phase in phases {
                    if let Some(items) = phase["items"].as_array_mut() { items.retain(|item| item.get("text").and_then(Value::as_str) != Some(task.as_str())); }
                }
            }
        } else {
            state["phases"] = json!([]);
        }
    } else if op != "view" {
        return Err(format!("unknown todo op: {op}"));
    }
    let summary = todo_summary(&mut state);
    fs::write(&file, serde_json::to_vec_pretty(&state).map_err(|e| e.to_string())?).map_err(|e| e.to_string())?;
    Ok(summary)
}

fn delegate_task(runtime: &ToolRuntime<'_>, input: &Value) -> Result<Value, String> {
    if !runtime.allow_command { return Err("delegate_task requires --allow-command".into()); }
    let task = string_input(input, "task").ok_or("delegate_task requires task")?;
    let max_steps = u64_input(input, "maxSteps", 6).clamp(1, 16).to_string();
    let exe = std::env::current_exe().map_err(|e| e.to_string())?;
    let result = run_process(runtime, &json!({"command": exe.display().to_string(), "args": ["run", task, "--cwd", runtime.cwd.display().to_string(), "--max-steps", max_steps, "--json"], "timeoutMs": 300_000}))?;
    let delegated = result.get("stdout").and_then(Value::as_str).and_then(|stdout| serde_json::from_str::<Value>(stdout).ok()).unwrap_or(Value::Null);
    let mut out = result;
    out["delegated"] = delegated;
    Ok(out)
}



fn mcp_timeout_ms(input: &Value) -> u64 {
    u64_input(input, "timeoutMs", 30_000).clamp(1_000, 120_000)
}

fn mcp_server(input: &Value) -> Result<String, String> {
    string_input(input, "server").filter(|server| !server.is_empty()).ok_or_else(|| "server is required".into())
}

fn mcp_list_tools(runtime: &ToolRuntime<'_>, input: &Value) -> Result<Value, String> {
    let server = mcp_server(input)?;
    crate::mcp::list_tools(runtime.cwd, &server, mcp_timeout_ms(input))
}

fn mcp_call_tool(runtime: &ToolRuntime<'_>, input: &Value) -> Result<Value, String> {
    let server = mcp_server(input)?;
    let tool = string_input(input, "tool").filter(|tool| !tool.is_empty()).ok_or_else(|| "tool is required".to_string())?;
    crate::mcp::call_tool(runtime.cwd, &server, &tool, object_input(input, "args"), mcp_timeout_ms(input))
}

fn mcp_list_resources(runtime: &ToolRuntime<'_>, input: &Value) -> Result<Value, String> {
    let server = mcp_server(input)?;
    crate::mcp::list_resources(runtime.cwd, &server, mcp_timeout_ms(input))
}

fn mcp_read_resource(runtime: &ToolRuntime<'_>, input: &Value) -> Result<Value, String> {
    let server = mcp_server(input)?;
    let uri = string_input(input, "uri").filter(|uri| !uri.is_empty()).ok_or_else(|| "uri is required".to_string())?;
    crate::mcp::read_resource(runtime.cwd, &server, &uri, mcp_timeout_ms(input))
}

fn mcp_list_prompts(runtime: &ToolRuntime<'_>, input: &Value) -> Result<Value, String> {
    let server = mcp_server(input)?;
    crate::mcp::list_prompts(runtime.cwd, &server, mcp_timeout_ms(input))
}

fn mcp_get_prompt(runtime: &ToolRuntime<'_>, input: &Value) -> Result<Value, String> {
    let server = mcp_server(input)?;
    let name = string_input(input, "name").filter(|name| !name.is_empty()).ok_or_else(|| "name is required".to_string())?;
    crate::mcp::get_prompt(runtime.cwd, &server, &name, object_input(input, "args"), mcp_timeout_ms(input))
}


pub fn execute(runtime: &ToolRuntime<'_>, tool: &str, input: &Value) -> Result<Value, String> {
    match tool {
        "list_dir" => list_dir(runtime, input),
        "read_file" => read_file(runtime, input),
        "read_binary_file" => read_binary_file(runtime, input),
        "search_text" => search_text(runtime, input),
        "search_files" => search_files(runtime, input),
        "glob_paths" => glob_paths(runtime, input),
        "read_image" => read_image(runtime, input),
        "read_sqlite" => read_sqlite(runtime, input),
        "grep_regex" => grep_regex(runtime, input),
        "write_file" => write_file(runtime, input),
        "delete_file" => delete_file(runtime, input),
        "move_file" => move_file(runtime, input),
        "run_command" => run_command(runtime, input),
        "run_process" => run_process(runtime, input),
        "node_eval" => node_eval(runtime, input),
        "python_eval" => python_eval(runtime, input),
        "list_package_scripts" => list_package_scripts(runtime),
        "run_package_script" => run_package_script(runtime, input),
        "git_status" => git_status(runtime),
        "git_diff" => git_diff(runtime, input),
        "todo" => todo_tool(runtime, input),
        "delegate_task" => delegate_task(runtime, input),
        "git_log" => git_log(runtime, input),
        "git_show" => git_show(runtime, input),
        "fetch_url" => fetch_url(runtime, input),
        "save_artifact" => save_artifact(runtime, input),
        "list_artifacts" => list_artifacts(runtime),
        "read_artifact" => read_artifact(runtime, input),
        "mcp_list_tools" => mcp_list_tools(runtime, input),
        "mcp_call_tool" => mcp_call_tool(runtime, input),
        "mcp_list_resources" => mcp_list_resources(runtime, input),
        "mcp_read_resource" => mcp_read_resource(runtime, input),
        "mcp_list_prompts" => mcp_list_prompts(runtime, input),
        "mcp_get_prompt" => mcp_get_prompt(runtime, input),
        other => Err(format!("Rust tool runtime has not ported tool: {other}")),
    }
}

pub fn format_tool_result(result: &Value) -> String {
    json!({"type": "tool_result", "result": result}).to_string()
}
