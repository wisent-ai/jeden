//! Turning one tool call into scheduler work: the `task`, `job` and `irc`
//! tools, and the synchronous `delegate` path that spawns a child and waits
//! for its output.

use super::scheduler::{BatchTask, SpawnRequest, TaskScheduler};
use super::types::TaskError;
use super::{default_store, limits_from_config};
use serde_json::{json, Value};
use std::fs;
use std::path::Path;

pub fn execute_delegate(
    cwd: &Path,
    artifact_dir: Option<&Path>,
    input: &Value,
) -> Result<Value, TaskError> {
    let scheduler = TaskScheduler::open(
        cwd,
        &default_store(cwd, artifact_dir),
        limits_from_config(cwd)?,
    )?;
    let mut request: SpawnRequest =
        serde_json::from_value(input.clone()).map_err(|e| TaskError::Invalid(e.to_string()))?;
    if request.agent.is_empty() {
        request.agent = "default".into();
    }
    if request.parent_job.is_none() {
        request.parent_job = running_job()?;
    }
    let job = scheduler.spawn(request)?;
    let job = scheduler.poll(&job.id)?;
    let stdout = bounded_text(&job.stdout, scheduler.limits.max_output_bytes)?;
    let stderr = bounded_text(&job.stderr, scheduler.limits.max_output_bytes)?;
    // The delegated answer is the child's JSON when it printed JSON, and its
    // printed text when it printed text; readers take either.
    let delegated = match serde_json::from_str::<Value>(stdout.trim()) {
        Ok(answer) => answer,
        Err(_text) => Value::String(stdout.clone()),
    };
    Ok(json!({"job":job,"stdout":stdout,"stderr":stderr,"delegated":delegated}))
}

/// The job this process runs as, from the `JEDEN_TASK_JOB` the scheduler
/// gives a child it starts: none outside a job, and a value that is not
/// Unicode refused naming the variable rather than read as no job.
fn running_job() -> Result<Option<String>, TaskError> {
    match std::env::var("JEDEN_TASK_JOB") {
        Ok(job) if job.is_empty() => Ok(None),
        Ok(job) => Ok(Some(job)),
        Err(std::env::VarError::NotPresent) => Ok(None),
        Err(std::env::VarError::NotUnicode(raw)) => Err(TaskError::Invalid(format!(
            "JEDEN_TASK_JOB is {raw:?}, which is not Unicode, so the parent job cannot be named"
        ))),
    }
}

pub(super) fn execute_task(scheduler: &TaskScheduler, input: &Value) -> Result<Value, TaskError> {
    if let Some(tasks) = input.get("tasks") {
        let batch: Vec<BatchTask> = serde_json::from_value(tasks.clone())?;
        return Ok(json!({"jobs":scheduler.batch(batch)?}));
    }
    let mut request: SpawnRequest = serde_json::from_value(input.clone())?;
    if request.parent_job.is_none() {
        request.parent_job = running_job()?;
    }
    let wait = match input.get("wait") {
        None => false,
        Some(Value::Bool(wait)) => *wait,
        Some(other) => {
            return Err(TaskError::Invalid(format!(
                "task wait is true or false; got {other}"
            )));
        }
    };
    let job = scheduler.spawn(request)?;
    if wait {
        Ok(json!(scheduler.poll(&job.id)?))
    } else {
        Ok(json!(job))
    }
}

pub(super) fn execute_job(scheduler: &TaskScheduler, input: &Value) -> Result<Value, TaskError> {
    let op = input
        .get("op")
        .and_then(Value::as_str)
        .ok_or_else(|| TaskError::Invalid("job op is required".into()))?;
    let id = || {
        input
            .get("id")
            .and_then(Value::as_str)
            .ok_or_else(|| TaskError::Invalid(format!("job {op} requires id")))
    };
    match op {
        "list" => Ok(json!(scheduler.list()?)),
        "poll" => Ok(json!(scheduler.poll(id()?)?)),
        "cancel" => Ok(json!({"cancelled":scheduler.cancel(id()?)?})),
        "deliver" => Ok(json!(scheduler.deliver(id()?)?)),
        "merge" => Ok(json!(scheduler.merge(id()?)?)),
        "health" => Ok(json!(scheduler.health())),
        _ => Err(TaskError::Invalid(format!("unknown job op: {op}"))),
    }
}

pub(super) fn execute_irc(scheduler: &TaskScheduler, input: &Value) -> Result<Value, TaskError> {
    let mailbox = scheduler.mailbox()?;
    let op = input
        .get("op")
        .and_then(Value::as_str)
        .ok_or_else(|| TaskError::Invalid("irc op is required".into()))?;
    let text = |key: &str| {
        input
            .get(key)
            .and_then(Value::as_str)
            .ok_or_else(|| TaskError::Invalid(format!("irc {op} requires {key}")))
    };
    match op {
        "send" => Ok(json!(mailbox.send(
            text("from")?,
            text("to")?,
            text("message")?,
            input
                .get("correlationId")
                .and_then(Value::as_str)
                .map(str::to_string),
            input
                .get("replyTo")
                .and_then(Value::as_str)
                .map(str::to_string)
        )?)),
        "inbox" => Ok(json!(mailbox.inbox(text("agent")?, true)?)),
        "wait" => Ok(json!(mailbox.wait(
            text("agent")?,
            input.get("correlationId").and_then(Value::as_str),
        )?)),
        "wake" => Ok(json!({"pending":mailbox.wake_pending(text("agent")?)?})),
        _ => Err(TaskError::Invalid(format!("unknown irc op: {op}"))),
    }
}

/// The text of the child's output file at `path`, at most `max` bytes of it
/// (the declared `maxOutputBytes`); a longer file is cut there and the text
/// says so, naming the file and its size.
fn bounded_text(path: &Path, max: u64) -> Result<String, TaskError> {
    use std::io::Read;
    let file = fs::File::open(path)?;
    let size = file.metadata()?.len();
    let mut bytes = Vec::new();
    file.take(max).read_to_end(&mut bytes)?;
    let mut text = String::from_utf8_lossy(&bytes).into_owned();
    if size > max {
        text.push_str(&format!(
            "\n[cut at maxOutputBytes {max} of the {size} bytes in {}]",
            path.display()
        ));
    }
    Ok(text)
}
