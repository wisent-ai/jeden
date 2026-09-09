//! Turning one tool call into scheduler work: the `task`, `job` and `irc`
//! tools, and the synchronous `delegate` path that spawns a child and waits
//! for its output.
//!
//! Split out of `task_runtime/mod.rs` because the operator's rule keeps every
//! source file at three hundred lines or fewer, and the write guard refuses
//! every edit to a file above that — including the field rename this split
//! made possible. It lives in its own folder because `task_runtime/` is
//! already at the five-files-per-folder limit the same guard enforces.

use super::scheduler::{BatchTask, SpawnRequest, TaskScheduler};
use super::types::TaskError;
use super::{default_store, limits_from_config};
use serde_json::{json, Value};
use std::fs;
use std::path::Path;
use std::time::Duration;

pub fn execute_delegate(
    cwd: &Path,
    artifact_dir: Option<&Path>,
    input: &Value,
) -> Result<Value, TaskError> {
    let scheduler = TaskScheduler::open(
        cwd,
        &default_store(cwd, artifact_dir),
        limits_from_config(cwd),
    )?;
    let mut request: SpawnRequest =
        serde_json::from_value(input.clone()).map_err(|e| TaskError::Invalid(e.to_string()))?;
    if request.agent.is_empty() {
        request.agent = "default".into();
    }
    if request.parent_job.is_none() {
        request.parent_job = std::env::var("JEDEN_TASK_JOB")
            .ok()
            .filter(|value| !value.is_empty());
    }
    let job = scheduler.spawn(request)?;
    let job = scheduler.poll(
        &job.id,
        Duration::from_millis(scheduler.limits.wait_budget_ms),
    )?;
    if !job.status.terminal() {
        let _ = scheduler.cancel(&job.id);
        return Err(TaskError::Timeout(format!(
            "delegated job exceeded its wait budget: {}",
            job.id
        )));
    }
    let stdout = bounded_text(&job.stdout, scheduler.limits.max_output_bytes)?;
    let stderr = bounded_text(&job.stderr, scheduler.limits.max_output_bytes)?;
    let delegated =
        serde_json::from_str::<Value>(stdout.trim()).unwrap_or(Value::String(stdout.clone()));
    Ok(json!({"job":job,"stdout":stdout,"stderr":stderr,"delegated":delegated}))
}

pub(super) fn execute_task(
    scheduler: &TaskScheduler,
    input: &Value,
) -> Result<Value, TaskError> {
    if let Some(tasks) = input.get("tasks") {
        let batch: Vec<BatchTask> = serde_json::from_value(tasks.clone())?;
        return Ok(json!({"jobs":scheduler.batch(batch)?}));
    }
    let mut request: SpawnRequest = serde_json::from_value(input.clone())?;
    if request.parent_job.is_none() {
        request.parent_job = std::env::var("JEDEN_TASK_JOB")
            .ok()
            .filter(|value| !value.is_empty());
    }
    let wait = input.get("wait").and_then(Value::as_bool).unwrap_or(false);
    let job = scheduler.spawn(request)?;
    if wait {
        Ok(json!(scheduler.poll(
            &job.id,
            Duration::from_millis(scheduler.limits.wait_budget_ms)
        )?))
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
        "poll" => Ok(json!(scheduler.poll(
            id()?,
            Duration::from_millis(
                input
                    .get("waitMs")
                    .and_then(Value::as_u64)
                    .unwrap_or(0)
                    .min(scheduler.limits.wait_budget_ms)
            )
        )?)),
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
            Duration::from_millis(
                input
                    .get("timeoutMs")
                    .and_then(Value::as_u64)
                    .unwrap_or(scheduler.limits.wait_budget_ms)
                    .min(scheduler.limits.wait_budget_ms)
            )
        )?)),
        "wake" => Ok(json!({"pending":mailbox.wake_pending(text("agent")?)?})),
        _ => Err(TaskError::Invalid(format!("unknown irc op: {op}"))),
    }
}

fn bounded_text(path: &Path, max: u64) -> Result<String, TaskError> {
    let file = fs::File::open(path)?;
    use std::io::Read;
    let mut bytes = Vec::new();
    file.take(max.saturating_add(1)).read_to_end(&mut bytes)?;
    if bytes.len() as u64 > max {
        bytes.truncate(max as usize);
    }
    Ok(String::from_utf8_lossy(&bytes).into_owned())
}
