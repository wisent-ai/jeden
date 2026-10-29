//! Running things: a command, a package script, a delegated task.

use serde_json::{json, Map, Value};
use std::ffi::OsString;
use std::fs;

use super::runtime_ops::{ManagedCommand, ManagedProcessResult, ProcessManager, TerminationReason};
use super::shared::string_input;
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
    Ok(process_result_json(
        result,
        Map::from_iter([("command".to_owned(), json!(command))]),
    ))
}

pub(crate) fn run_process(runtime: &ToolRuntime<'_>, input: &Value) -> Result<Value, String> {
    if !runtime.allow_command {
        return Err("run_process requires --allow-command".into());
    }
    let command = string_input(input, "command").ok_or("run_process requires command")?;
    // The schema declares args an array of strings and env an object of
    // strings (null unsets one); any other value is refused by name, never
    // turned into its JSON text and handed to the process.
    let args = match input.get("args") {
        None | Some(Value::Null) => Vec::new(),
        Some(Value::Array(values)) => values
            .iter()
            .enumerate()
            .map(|(at, value)| match value.as_str() {
                Some(text) => Ok(text.to_string()),
                None => Err(format!("run_process args[{at}] is {value}, not a string")),
            })
            .collect::<Result<Vec<_>, String>>()?,
        Some(other) => {
            return Err(format!(
                "run_process args is {other}, not an array of strings"
            ))
        }
    };

    let mut managed = ManagedCommand::new(&command, runtime.cwd);
    managed.args = args.iter().map(OsString::from).collect();
    managed.stdin = string_input(input, "stdin").map(String::into_bytes);
    let env = match input.get("env") {
        None | Some(Value::Null) => None,
        Some(Value::Object(env)) => Some(env),
        Some(other) => {
            return Err(format!(
                "run_process env is {other}, not an object of strings"
            ))
        }
    };
    if let Some(env) = env {
        managed.env.reserve(env.len());
        for (key, value) in env {
            let value = match value {
                Value::Null => None,
                Value::String(text) => Some(OsString::from(text)),
                other => {
                    return Err(format!(
                        "run_process env.{key} is {other}, not a string or null"
                    ))
                }
            };
            managed.env.push((OsString::from(key), value));
        }
    }
    let result = ProcessManager.run(&runtime.operation, managed)?;
    Ok(process_result_json(
        result,
        Map::from_iter([
            ("command".to_owned(), json!(command)),
            ("args".to_owned(), json!(args)),
        ]),
    ))
}

/// `object` (what was run) with the process's outcome beside it. It is a JSON
/// object by type, so no caller can hand one the outcome cannot join.
pub(super) fn process_result_json(
    result: ManagedProcessResult,
    mut object: Map<String, Value>,
) -> Value {
    let cancelled = result.reason == TerminationReason::Cancelled;
    let completed = result.reason == TerminationReason::Completed;
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
        json!(result
            .stdout
            .artifact
            .map(|path| path.display().to_string())),
    );
    object.insert("stdoutSha256".into(), json!(result.stdout.sha256));
    object.insert("stderrHead".into(), json!(result.stderr.head));
    object.insert("stderrTail".into(), json!(result.stderr.tail));
    object.insert("stderrBytes".into(), json!(result.stderr.total_bytes));
    object.insert("stderrTruncated".into(), json!(result.stderr.truncated));
    object.insert(
        "stderrArtifact".into(),
        json!(result
            .stderr
            .artifact
            .map(|path| path.display().to_string())),
    );
    object.insert("stderrSha256".into(), json!(result.stderr.sha256));
    Value::Object(object)
}

pub(crate) fn list_package_scripts(runtime: &ToolRuntime<'_>) -> Result<Value, String> {
    let file = runtime.cwd.join("package.json");
    let raw =
        fs::read_to_string(&file).map_err(|e| format!("{} cannot be read: {e}", file.display()))?;
    let parsed: Value =
        serde_json::from_str(&raw).map_err(|e| format!("{} is not JSON: {e}", file.display()))?;
    let mut scripts = serde_json::Map::new();
    if let Some(raw_scripts) = parsed.get("scripts").and_then(Value::as_object) {
        for (name, value) in raw_scripts {
            let Some(script) = value.as_str() else {
                return Err(format!(
                    "{} declares script {name} as {value}, not a command string",
                    file.display()
                ));
            };
            scripts.insert(name.clone(), json!(script));
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
