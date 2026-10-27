//! Background work started from a session, and the record of what happened
//! to it.
//!
//! Split out of `slash/session/mod.rs`, which had grown past the module line
//! cap.

use crate::slash::session::slash_session_dir;
use crate::slash::SlashContext;
use crate::tui::{PickerItem, PickerSpec};
use serde_json::Value;
use std::path::PathBuf;

fn task_scheduler(
    context: &SlashContext<'_>,
) -> Result<crate::task_runtime::TaskScheduler, String> {
    let session_dir = slash_session_dir(context, "")?;
    let limits =
        crate::task_runtime::limits_from_config(context.cwd).map_err(|error| error.to_string())?;
    crate::task_runtime::TaskScheduler::open(context.cwd, &session_dir.join("task-runtime"), limits)
        .map_err(|error| error.to_string())
}

pub(crate) fn handle_tan(args: &str, context: &SlashContext<'_>) -> Result<String, String> {
    let task = args.trim();
    if task.is_empty() {
        return Err("Usage: /tan <work>".into());
    }
    let scheduler = task_scheduler(context)?;
    // The job this session runs inside, when it is one: a value that is not
    // Unicode is refused, never read as no parent.
    let parent_job = match std::env::var("JEDEN_TASK_JOB") {
        Ok(value) if !value.is_empty() => Some(value),
        Ok(_) | Err(std::env::VarError::NotPresent) => None,
        Err(std::env::VarError::NotUnicode(_)) => {
            return Err("JEDEN_TASK_JOB cannot be read: value is not Unicode".into())
        }
    };
    let job = scheduler
        .spawn(crate::task_runtime::SpawnRequest {
            task: task.into(),
            agent: "default".into(),
            model: context
                .model
                .filter(|model| !model.trim().is_empty())
                .map(str::to_string),
            max_steps: None,
            parent_job,
            isolate: Some(true),
        })
        .map_err(|error| error.to_string())?;
    let pid = match job.pid {
        Some(pid) => pid.to_string(),
        None => "none yet, queued".into(),
    };
    Ok(format!(
        "Started durable task job {}.\nPID: {pid}\nWorkspace: {} ({})\nSession: {}\nUse /jobs to poll it.",
        job.id,
        job.workspace.display(),
        job.isolation,
        job.session_path.display(),
    ))
}

/// The jobs directory and every tracked job, or why they cannot be listed.
fn tracked_jobs(context: &SlashContext<'_>) -> Result<(PathBuf, Vec<Value>), String> {
    let scheduler = task_scheduler(context)
        .map_err(|error| format!("the task scheduler cannot be opened: {error}"))?;
    let dir = scheduler.store.join("jobs");
    let jobs = scheduler
        .list()
        .map_err(|error| format!("the jobs in {} cannot be listed: {error}", dir.display()))?
        .into_iter()
        .map(|job| {
            serde_json::to_value(job).map_err(|error| format!("a job cannot be shown: {error}"))
        })
        .collect::<Result<Vec<_>, _>>()?;
    Ok((dir, jobs))
}

pub(crate) fn jobs_picker(context: &SlashContext<'_>) -> PickerSpec {
    let (dir, jobs) = match tracked_jobs(context) {
        Ok(listed) => listed,
        Err(cause) => {
            return PickerSpec::new(
                "Background jobs",
                vec![PickerItem::action("Background jobs cannot be listed", "")
                    .detail(cause)
                    .badge("error")
                    .disabled(true)],
            );
        }
    };
    let items = if jobs.is_empty() {
        vec![PickerItem::action("No tracked background jobs", "")
            .detail(format!("Job metadata directory: {}", dir.display()))
            .badge("empty")
            .disabled(true)]
    } else {
        jobs.into_iter()
            .map(|job| {
                let field = |key: &str| match job.get(key) {
                    Some(Value::String(text)) => text.clone(),
                    Some(Value::Null) | None => "not recorded".to_string(),
                    Some(other) => other.to_string(),
                };
                let (id, recorded_status, pid) = (field("id"), field("status"), field("pid"));
                let task = match job.get("task").and_then(Value::as_str) {
                    Some(task) => task.to_string(),
                    None => String::new(),
                };
                PickerItem::action(&id, format!("/copy {task}"))
                    .detail(format!(
                        "{task} — recorded PID {pid}; Enter copies the task"
                    ))
                    .badge(format!("recorded {recorded_status}"))
                    .disabled(task.is_empty())
            })
            .collect()
    };
    PickerSpec::new("Background jobs", items)
}

pub(crate) fn handle_jobs(context: &SlashContext<'_>) -> Result<String, String> {
    let (dir, jobs) = tracked_jobs(context)?;
    if jobs.is_empty() {
        Ok(format!(
            "No background jobs are tracked in {}.",
            dir.display()
        ))
    } else {
        serde_json::to_string_pretty(&jobs).map_err(|e| e.to_string())
    }
}
