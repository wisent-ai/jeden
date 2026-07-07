use serde_json::{json, Value};
use std::fs;
use std::io::Write;
use std::process::{Command, Stdio};
use std::thread::sleep;
use std::time::{Duration, Instant};

use super::shared::{jail_path, run_read_process, sha256_hex, string_input, u64_input, line_window};
use super::ToolRuntime;

mod search;

pub(crate) use search::{glob_paths, grep_regex, search_files, search_text};

pub(crate) fn run_command(runtime: &ToolRuntime<'_>, input: &Value) -> Result<Value, String> {
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

pub(crate) fn run_process(runtime: &ToolRuntime<'_>, input: &Value) -> Result<Value, String> {
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

pub(crate) fn node_eval(runtime: &ToolRuntime<'_>, input: &Value) -> Result<Value, String> {
    let code = string_input(input, "code").ok_or("node_eval requires code")?;
    run_process(runtime, &json!({"command": "node", "args": ["--input-type=module", "-"], "stdin": code, "timeoutMs": u64_input(input, "timeoutMs", 30_000)}))
}

pub(crate) fn python_eval(runtime: &ToolRuntime<'_>, input: &Value) -> Result<Value, String> {
    let code = string_input(input, "code").ok_or("python_eval requires code")?;
    run_process(runtime, &json!({"command": "python3", "args": ["-"], "stdin": code, "timeoutMs": u64_input(input, "timeoutMs", 30_000)}))
}

pub(crate) fn list_package_scripts(runtime: &ToolRuntime<'_>) -> Result<Value, String> {
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

pub(crate) fn run_package_script(runtime: &ToolRuntime<'_>, input: &Value) -> Result<Value, String> {
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

pub(crate) fn git_status(runtime: &ToolRuntime<'_>) -> Result<Value, String> {
    run_read_process(runtime, &json!({"command": "git", "args": ["status", "--short"], "timeoutMs": 30_000}))
}

pub(crate) fn git_diff(runtime: &ToolRuntime<'_>, input: &Value) -> Result<Value, String> {
    let mut args = vec!["diff".to_string(), "--".to_string()];
    if let Some(path) = string_input(input, "path") {
        let _ = jail_path(runtime.cwd, &path)?;
        args.push(path);
    }
    run_read_process(runtime, &json!({"command": "git", "args": args, "timeoutMs": 30_000}))
}

pub(crate) fn git_log(runtime: &ToolRuntime<'_>, input: &Value) -> Result<Value, String> {
    let limit = u64_input(input, "limit", 20).clamp(1, 100);
    let mut args = vec!["log".to_string(), format!("-{limit}"), "--oneline".to_string(), "--decorate".to_string(), "--".to_string()];
    if let Some(path) = string_input(input, "path") {
        let _ = jail_path(runtime.cwd, &path)?;
        args.push(path);
    }
    run_read_process(runtime, &json!({"command": "git", "args": args, "timeoutMs": 30_000}))
}

pub(crate) fn git_show(runtime: &ToolRuntime<'_>, input: &Value) -> Result<Value, String> {
    let reference = string_input(input, "ref").unwrap_or_else(|| "HEAD".into());
    let mut args = vec!["show".to_string(), "--stat".to_string(), "--oneline".to_string(), "--decorate".to_string(), reference, "--".to_string()];
    if let Some(path) = string_input(input, "path") {
        let _ = jail_path(runtime.cwd, &path)?;
        args.push(path);
    }
    run_read_process(runtime, &json!({"command": "git", "args": args, "timeoutMs": 30_000}))
}

pub(crate) fn fetch_url(_runtime: &ToolRuntime<'_>, input: &Value) -> Result<Value, String> {
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

pub(crate) fn delegate_task(runtime: &ToolRuntime<'_>, input: &Value) -> Result<Value, String> {
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
