//! Evaluating code in a persistent interpreter, and driving a real shell.
//!
//! Split out of `tool_runtime/exec/mod.rs`, which had grown past the module
//! line cap.

use super::super::runtime_ops::{
    kernel::{self, KernelLanguage},
    pty,
};
use super::super::shared::{bool_input, string_input};
use super::super::ToolRuntime;
use serde_json::{json, Value};

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
    let reset = bool_input(input, "reset", false);
    let scope = runtime.artifact_dir.unwrap_or(runtime.cwd);
    let result = kernel::evaluate(
        &runtime.operation,
        scope,
        runtime.cwd,
        language,
        &code,
        reset,
    )?;
    Ok(json!({
        "ok": result.ok,
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
    let reset = bool_input(input, "reset", false);
    let scope = runtime.artifact_dir.unwrap_or(runtime.cwd);
    let result = pty::execute(&runtime.operation, scope, runtime.cwd, &command, reset)?;
    Ok(json!({
        "ok": result.ok,
        "command": command,
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
