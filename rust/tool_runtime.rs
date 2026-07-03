use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::fs;
use std::path::{Component, Path, PathBuf};
use std::process::{Command, Stdio};
use std::thread::sleep;
use std::time::{Duration, Instant};

const MAX_READ_BYTES: u64 = 512_000;

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

pub fn execute(runtime: &ToolRuntime<'_>, tool: &str, input: &Value) -> Result<Value, String> {
    match tool {
        "list_dir" => list_dir(runtime, input),
        "read_file" => read_file(runtime, input),
        "search_text" => search_text(runtime, input),
        "write_file" => write_file(runtime, input),
        "run_command" => run_command(runtime, input),
        "save_artifact" => save_artifact(runtime, input),
        other => Err(format!("Rust tool runtime has not ported tool: {other}")),
    }
}

pub fn format_tool_result(result: &Value) -> String {
    json!({"type": "tool_result", "result": result}).to_string()
}
