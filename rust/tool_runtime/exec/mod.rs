use serde_json::{json, Value};
use std::ffi::OsString;
use std::fs;
use std::io::Read;
use std::time::Duration;

use super::runtime_ops::{
    kernel::{self, KernelLanguage},
    pty, BoundedOutput, ManagedCommand, ManagedProcessResult, OperationProgress, OutputLimits,
    ProcessManager, TerminationReason,
};
use super::shared::{
    bool_input, jail_path, line_window, run_read_process, string_input, u64_input,
};
use super::ToolRuntime;

mod search;

pub(crate) use search::{glob_paths, grep_regex, search_files, search_text};

pub(crate) fn run_command(runtime: &ToolRuntime<'_>, input: &Value) -> Result<Value, String> {
    if !runtime.allow_command {
        return Err("run_command requires --allow-command".into());
    }
    let command = string_input(input, "command").ok_or("run_command requires command")?;
    let timeout_ms = u64_input(input, "timeoutMs", 30_000).min(120_000);
    let mut managed = ManagedCommand::new("sh", runtime.cwd);
    managed.args = vec![OsString::from("-c"), OsString::from(&command)];
    let result = ProcessManager.run(
        &runtime.operation,
        managed,
        Duration::from_millis(timeout_ms),
    )?;
    Ok(process_result_json(
        result,
        json!({"command": command, "timeoutMs": timeout_ms}),
    ))
}

pub(crate) fn run_process(runtime: &ToolRuntime<'_>, input: &Value) -> Result<Value, String> {
    if !runtime.allow_command {
        return Err("run_process requires --allow-command".into());
    }
    let command = string_input(input, "command").ok_or("run_process requires command")?;
    let args = input
        .get("args")
        .and_then(Value::as_array)
        .map(|values| {
            values
                .iter()
                .map(|value| {
                    value
                        .as_str()
                        .map(ToString::to_string)
                        .unwrap_or_else(|| value.to_string())
                })
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    let timeout_ms = u64_input(input, "timeoutMs", 30_000).clamp(1_000, 120_000);
    let mut managed = ManagedCommand::new(&command, runtime.cwd);
    managed.args = args.iter().map(OsString::from).collect();
    managed.stdin = string_input(input, "stdin").map(String::into_bytes);
    if let Some(env) = input.get("env").and_then(Value::as_object) {
        managed.env.reserve(env.len());
        for (key, value) in env {
            let value = if value.is_null() {
                None
            } else {
                Some(OsString::from(
                    value
                        .as_str()
                        .map(ToString::to_string)
                        .unwrap_or_else(|| value.to_string()),
                ))
            };
            managed.env.push((OsString::from(key), value));
        }
    }
    let result = ProcessManager.run(
        &runtime.operation,
        managed,
        Duration::from_millis(timeout_ms),
    )?;
    Ok(process_result_json(
        result,
        json!({"command": command, "args": args, "timeoutMs": timeout_ms}),
    ))
}

fn process_result_json(result: ManagedProcessResult, mut base: Value) -> Value {
    let timed_out = result.reason == TerminationReason::TimedOut;
    let cancelled = result.reason == TerminationReason::Cancelled;
    let completed = result.reason == TerminationReason::Completed;
    let object = base
        .as_object_mut()
        .expect("process result base must be an object");
    object.insert("ok".into(), json!(completed && result.status.success()));
    object.insert("timedOut".into(), json!(timed_out));
    object.insert("cancelled".into(), json!(cancelled));
    object.insert("code".into(), json!(result.status.code()));
    object.insert("stdout".into(), json!(result.stdout.text));
    object.insert("stderr".into(), json!(result.stderr.text));
    object.insert("stdoutHead".into(), json!(result.stdout.head));
    object.insert("stdoutTail".into(), json!(result.stdout.tail));
    object.insert("stdoutBytes".into(), json!(result.stdout.total_bytes));
    object.insert("stdoutTruncated".into(), json!(result.stdout.truncated));
    object.insert(
        "stdoutArtifact".into(),
        result
            .stdout
            .artifact
            .map(|path| json!(path.display().to_string()))
            .unwrap_or(Value::Null),
    );
    object.insert("stdoutSha256".into(), json!(result.stdout.sha256));
    object.insert("stderrHead".into(), json!(result.stderr.head));
    object.insert("stderrTail".into(), json!(result.stderr.tail));
    object.insert("stderrBytes".into(), json!(result.stderr.total_bytes));
    object.insert("stderrTruncated".into(), json!(result.stderr.truncated));
    object.insert(
        "stderrArtifact".into(),
        result
            .stderr
            .artifact
            .map(|path| json!(path.display().to_string()))
            .unwrap_or(Value::Null),
    );
    object.insert("stderrSha256".into(), json!(result.stderr.sha256));
    base
}

pub(crate) fn node_eval(runtime: &ToolRuntime<'_>, input: &Value) -> Result<Value, String> {
    eval_with_language(runtime, input, KernelLanguage::JavaScript, "node_eval")
}

pub(crate) fn python_eval(runtime: &ToolRuntime<'_>, input: &Value) -> Result<Value, String> {
    eval_with_language(runtime, input, KernelLanguage::Python, "python_eval")
}

pub(crate) fn eval_session(runtime: &ToolRuntime<'_>, input: &Value) -> Result<Value, String> {
    let language = KernelLanguage::parse(
        &string_input(input, "language").ok_or("eval_session requires language")?,
    )?;
    eval_with_language(runtime, input, language, "eval_session")
}

fn eval_with_language(
    runtime: &ToolRuntime<'_>,
    input: &Value,
    language: KernelLanguage,
    tool: &str,
) -> Result<Value, String> {
    if !runtime.allow_command {
        return Err(format!("{tool} requires --allow-command"));
    }
    let code = string_input(input, "code").ok_or_else(|| format!("{tool} requires code"))?;
    let timeout_ms = u64_input(input, "timeoutMs", 30_000).clamp(1_000, 120_000);
    let reset = bool_input(input, "reset", false);
    let scope = runtime.artifact_dir.unwrap_or(runtime.cwd);
    let result = kernel::evaluate(
        &runtime.operation,
        scope,
        runtime.cwd,
        language,
        &code,
        reset,
        Duration::from_millis(timeout_ms),
    )?;
    Ok(json!({
        "ok": result.ok,
        "timeoutMs": timeout_ms,
        "timedOut": result.timed_out,
        "cancelled": result.cancelled,
        "code": Value::Null,
        "stdout": result.stdout.text,
        "stderr": result.stderr.text,
        "stdoutHead": result.stdout.head,
        "stdoutTail": result.stdout.tail,
        "stdoutBytes": result.stdout.total_bytes,
        "stdoutTruncated": result.stdout.truncated,
        "stdoutArtifact": result.stdout.artifact.map(|path| path.display().to_string()),
        "stderrHead": result.stderr.head,
        "stderrTail": result.stderr.tail,
        "stderrBytes": result.stderr.total_bytes,
        "stderrTruncated": result.stderr.truncated,
        "stderrArtifact": result.stderr.artifact.map(|path| path.display().to_string()),
        "display": result.display.text,
        "displayMime": result.display_mime,
        "displayBytes": result.display.total_bytes,
        "displayTruncated": result.display.truncated,
        "displayArtifact": result.display.artifact.map(|path| path.display().to_string()),
        "error": result.error,
        "persistent": true,
        "reset": result.reset
    }))
}

pub(crate) fn pty_session(runtime: &ToolRuntime<'_>, input: &Value) -> Result<Value, String> {
    if !runtime.allow_command {
        return Err("pty_session requires --allow-command".into());
    }
    let command = string_input(input, "input")
        .or_else(|| string_input(input, "command"))
        .ok_or("pty_session requires input")?;
    let timeout_ms = u64_input(input, "timeoutMs", 30_000).clamp(1_000, 120_000);
    let reset = bool_input(input, "reset", false);
    let scope = runtime.artifact_dir.unwrap_or(runtime.cwd);
    let result = pty::execute(
        &runtime.operation,
        scope,
        runtime.cwd,
        &command,
        reset,
        Duration::from_millis(timeout_ms),
    )?;
    Ok(json!({
        "ok": result.ok,
        "command": command,
        "timeoutMs": timeout_ms,
        "timedOut": result.timed_out,
        "cancelled": result.cancelled,
        "code": result.code,
        "stdout": result.output.text,
        "stderr": "",
        "stdoutHead": result.output.head,
        "stdoutTail": result.output.tail,
        "stdoutBytes": result.output.total_bytes,
        "stdoutTruncated": result.output.truncated,
        "stdoutArtifact": result.output.artifact.map(|path| path.display().to_string()),
        "persistent": true,
        "sessionId": result.session.session_id,
        "cols": result.session.cols,
        "rows": result.session.rows,
        "reset": result.reset
    }))
}
pub(crate) fn pty_resize(runtime: &ToolRuntime<'_>, input: &Value) -> Result<Value, String> {
    if !runtime.allow_command {
        return Err("pty_resize requires --allow-command".into());
    }
    let session_id = string_input(input, "sessionId").ok_or("pty_resize requires sessionId")?;
    let cols_value = input
        .get("cols")
        .and_then(Value::as_u64)
        .ok_or("pty_resize requires integer cols")?;
    let rows_value = input
        .get("rows")
        .and_then(Value::as_u64)
        .ok_or("pty_resize requires integer rows")?;
    let cols = u16::try_from(cols_value).unwrap_or(u16::MAX);
    let rows = u16::try_from(rows_value).unwrap_or(u16::MAX);
    let session = pty::resize(&runtime.operation, &session_id, cols, rows)
        .map_err(|error| error.to_string())?;
    Ok(json!({
        "ok": true,
        "sessionId": session.session_id,
        "cols": session.cols,
        "rows": session.rows,
        "state": "live"
    }))
}

pub(crate) fn list_package_scripts(runtime: &ToolRuntime<'_>) -> Result<Value, String> {
    let file = runtime.cwd.join("package.json");
    let raw = fs::read_to_string(&file).map_err(|e| e.to_string())?;
    let parsed: Value = serde_json::from_str(&raw).map_err(|e| e.to_string())?;
    let mut scripts = serde_json::Map::new();
    if let Some(raw_scripts) = parsed.get("scripts").and_then(Value::as_object) {
        for (name, value) in raw_scripts {
            if let Some(script) = value.as_str() {
                scripts.insert(name.clone(), json!(script));
            }
        }
    }
    Ok(Value::Object(scripts))
}

pub(crate) fn run_package_script(
    runtime: &ToolRuntime<'_>,
    input: &Value,
) -> Result<Value, String> {
    if !runtime.allow_command {
        return Err("run_package_script requires --allow-command".into());
    }
    let script = string_input(input, "script").ok_or("run_package_script requires script")?;
    let scripts = list_package_scripts(runtime)?;
    if scripts.get(&script).and_then(Value::as_str).is_none() {
        return Err(format!("unknown package script: {script}"));
    }
    let mut payload = json!({"command": "npm", "args": ["run", script], "timeoutMs": u64_input(input, "timeoutMs", 60_000).clamp(1_000, 180_000)});
    if let Some(env) = input.get("env") {
        payload["env"] = env.clone();
    }
    run_process(runtime, &payload)
}

pub(crate) fn git_status(runtime: &ToolRuntime<'_>) -> Result<Value, String> {
    run_read_process(
        runtime,
        &json!({"command": "git", "args": ["status", "--short"], "timeoutMs": 30_000}),
    )
}

pub(crate) fn git_diff(runtime: &ToolRuntime<'_>, input: &Value) -> Result<Value, String> {
    let mut args = vec!["diff".to_string(), "--".to_string()];
    if let Some(path) = string_input(input, "path") {
        let _ = jail_path(runtime.cwd, &path)?;
        args.push(path);
    }
    run_read_process(
        runtime,
        &json!({"command": "git", "args": args, "timeoutMs": 30_000}),
    )
}

pub(crate) fn git_log(runtime: &ToolRuntime<'_>, input: &Value) -> Result<Value, String> {
    let limit = u64_input(input, "limit", 20).clamp(1, 100);
    let mut args = vec![
        "log".to_string(),
        format!("-{limit}"),
        "--oneline".to_string(),
        "--decorate".to_string(),
        "--".to_string(),
    ];
    if let Some(path) = string_input(input, "path") {
        let _ = jail_path(runtime.cwd, &path)?;
        args.push(path);
    }
    run_read_process(
        runtime,
        &json!({"command": "git", "args": args, "timeoutMs": 30_000}),
    )
}

pub(crate) fn git_show(runtime: &ToolRuntime<'_>, input: &Value) -> Result<Value, String> {
    let reference = string_input(input, "ref").unwrap_or_else(|| "HEAD".into());
    let mut args = vec![
        "show".to_string(),
        "--stat".to_string(),
        "--oneline".to_string(),
        "--decorate".to_string(),
        reference,
        "--".to_string(),
    ];
    if let Some(path) = string_input(input, "path") {
        let _ = jail_path(runtime.cwd, &path)?;
        args.push(path);
    }
    run_read_process(
        runtime,
        &json!({"command": "git", "args": args, "timeoutMs": 30_000}),
    )
}

pub(crate) fn fetch_url(runtime: &ToolRuntime<'_>, input: &Value) -> Result<Value, String> {
    let url = string_input(input, "url").ok_or("fetch_url requires url")?;
    if !url.starts_with("http://") && !url.starts_with("https://") {
        return Err("fetch_url requires http(s) URL".into());
    }
    let timeout_ms = u64_input(input, "timeoutMs", 30_000).clamp(1_000, 120_000);
    let max_bytes = u64_input(input, "maxBytes", 200_000).clamp(1_000, 1_000_000) as usize;
    let request_deadline = runtime
        .operation
        .effective_deadline(Duration::from_millis(timeout_ms));
    if runtime.operation.cancellation().is_cancelled() {
        return Err("fetch_url cancelled".into());
    }
    if std::time::Instant::now() >= request_deadline {
        return Err("fetch_url timed out".into());
    }
    let client = reqwest::blocking::Client::builder()
        .timeout(request_deadline.saturating_duration_since(std::time::Instant::now()))
        .build()
        .map_err(|e| e.to_string())?;
    let mut response = client.get(&url).send().map_err(|e| e.to_string())?;
    let status = response.status().as_u16();
    let content_type = response
        .headers()
        .get(reqwest::header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .map(ToString::to_string);
    let mut capture = BoundedOutput::new(
        "fetch",
        OutputLimits {
            head_bytes: max_bytes / 2,
            tail_bytes: max_bytes - (max_bytes / 2),
        },
        runtime.operation.artifacts().clone(),
    );
    let mut buffer = [0u8; 8192];
    let mut total = 0u64;
    loop {
        if runtime.operation.cancellation().is_cancelled() {
            return Err("fetch_url cancelled".into());
        }
        if std::time::Instant::now() >= request_deadline {
            return Err("fetch_url timed out".into());
        }
        let count = response.read(&mut buffer).map_err(|e| e.to_string())?;
        if count == 0 {
            break;
        }
        capture
            .write_chunk(&buffer[..count])
            .map_err(|e| format!("failed capturing fetch response: {e}"))?;
        total = total.saturating_add(count as u64);
        runtime.operation.progress(OperationProgress {
            stream: "fetch",
            bytes: count as u64,
            total_bytes: total,
        });
    }
    let captured = capture.finish().map_err(|e| e.to_string())?;
    let artifact = captured
        .artifact
        .as_ref()
        .map(|path| path.display().to_string());
    if captured.truncated && input.get("range").is_some() {
        return Err(format!(
            "fetch_url cannot apply a line range beyond maxBytes; full response saved at {}",
            artifact.as_deref().unwrap_or("artifact sink")
        ));
    }
    let (text, start_line, end_line, ranges) = if let Some(range) = string_input(input, "range") {
        line_window(&captured.text, &range)?
    } else {
        (captured.text, 0, 0, Vec::new())
    };
    Ok(json!({
        "ok": status >= 200 && status < 300,
        "url": url,
        "status": status,
        "contentType": content_type,
        "bytes": captured.total_bytes,
        "truncated": captured.truncated,
        "sha256": captured.sha256,
        "text": text,
        "head": captured.head,
        "tail": captured.tail,
        "artifact": artifact,
        "startLine": if ranges.is_empty() { Value::Null } else { json!(start_line) },
        "endLine": if ranges.is_empty() { Value::Null } else { json!(end_line) },
        "ranges": if ranges.is_empty() { Value::Null } else { json!(ranges) }
    }))
}

pub(crate) fn delegate_task(runtime: &ToolRuntime<'_>, input: &Value) -> Result<Value, String> {
    if !runtime.allow_command {
        return Err("delegate_task requires --allow-command".into());
    }
    if runtime.operation.cancellation().is_cancelled() {
        return Err("delegate_task cancelled before scheduling".into());
    }
    crate::tool_runtime::runtime_ops::untrusted_child(
        &runtime.operation,
        format!("{}:delegate-task", runtime.operation.operation_id()),
    )
    .map_err(|error| error.to_string())?;
    crate::task_runtime::execute_delegate(runtime.cwd, runtime.artifact_dir, input)
        .map_err(|error| error.to_string())
}
