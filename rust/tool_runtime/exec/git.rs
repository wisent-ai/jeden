//! Asking the repository in the workspace what its state is, through the
//! version control program itself.
//!
//! Split out of `tool_runtime/exec/mod.rs`, which had grown past the module
//! line cap.

use super::super::shared::{jail_path, run_read_process, string_input, u64_input};
use super::super::ToolRuntime;
use serde_json::{json, Value};

pub(crate) fn git_status(runtime: &ToolRuntime<'_>) -> Result<Value, String> {
    run_read_process(
        runtime,
        &json!({"command": "git", "args": ["status", "--short"]}),
    )
}

pub(crate) fn git_diff(runtime: &ToolRuntime<'_>, input: &Value) -> Result<Value, String> {
    let mut args = vec!["diff".to_string(), "--".to_string()];
    if let Some(path) = string_input(input, "path") {
        let _ = jail_path(runtime.cwd, &path)?;
        args.push(path);
    }
    run_read_process(runtime, &json!({"command": "git", "args": args}))
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
    run_read_process(runtime, &json!({"command": "git", "args": args}))
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
    run_read_process(runtime, &json!({"command": "git", "args": args}))
}
