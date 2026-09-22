//! Running things: a command, a package script, a delegated task.

use serde_json::{json, Value};
use std::ffi::OsString;
use std::fs;

use super::runtime_ops::{
    BoundedOutput, ManagedCommand, ManagedProcessResult, OperationProgress, OutputLimits,
    ProcessManager, TerminationReason,
};
use super::shared::{bool_input, jail_path, string_input, u64_input};
use super::ToolRuntime;

mod git;
mod kernels;
mod search;
mod web;

pub(crate) use git::{git_diff, git_log, git_show, git_status};
pub(crate) use kernels::{eval_session, node_eval, pty_resize, pty_session, python_eval};
pub(crate) use search::{glob_paths, grep_regex, search_files, search_text};
pub(crate) use web::fetch_url;

pub(crate) fn run_command(runtime: &ToolRuntime<'_>, input: &Value) -> Result<Value, String> {
    if !runtime.allow_command {
        return Err("run_command requires --allow-command".into());
    }
    let command = string_input(input, "command").ok_or("run_command requires command")?;
    let mut managed = ManagedCommand::new("sh", runtime.cwd);
    managed.args = vec![OsString::from("-c"), OsString::from(&command)];
    let result = ProcessManager.run(&runtime.operation, managed)?;
    Ok(process_result_json(result, json!({"command": command})))
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
    let result = ProcessManager.run(&runtime.operation, managed)?;
    Ok(process_result_json(
        result,
        json!({"command": command, "args": args}),
    ))
}

pub(super) fn process_result_json(result: ManagedProcessResult, mut base: Value) -> Value {
    let cancelled = result.reason == TerminationReason::Cancelled;
    let completed = result.reason == TerminationReason::Completed;
    let object = base
        .as_object_mut()
        .expect("process result base must be an object");
    object.insert("ok".into(), json!(completed && result.status.success()));
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
    let mut payload = json!({"command": "npm", "args": ["run", script]});
    if let Some(env) = input.get("env") {
        payload["env"] = env.clone();
    }
    run_process(runtime, &payload)
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
