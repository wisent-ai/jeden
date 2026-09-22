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
    crate::task_runtime::TaskScheduler::open(
        context.cwd,
        &session_dir.join("task-runtime"),
        crate::task_runtime::limits_from_config(context.cwd),
    )
    .map_err(|error| error.to_string())
}

pub(crate) fn handle_tan(args: &str, context: &SlashContext<'_>) -> Result<String, String> {
    let task = args.trim();
    if task.is_empty() {
        return Err("Usage: /tan <work>".into());
    }
    let scheduler = task_scheduler(context)?;
    let job = scheduler
        .spawn(crate::task_runtime::SpawnRequest {
            task: task.into(),
            agent: "default".into(),
            model: context
                .model
                .filter(|model| !model.trim().is_empty())
                .map(str::to_string),
            max_steps: 6,
            parent_job: std::env::var("JEDEN_TASK_JOB")
                .ok()
                .filter(|value| !value.is_empty()),
            isolate: Some(true),
        })
        .map_err(|error| error.to_string())?;
    Ok(format!(
        "Started durable task job {}.\nPID: {}\nWorkspace: {} ({})\nSession: {}\nUse /jobs to poll it.",
        job.id,
        job.pid.map(|pid| pid.to_string()).unwrap_or_else(|| "queued".into()),
        job.workspace.display(),
        job.isolation,
        job.session_path.display(),
    ))
}

fn tracked_jobs(context: &SlashContext<'_>) -> Option<(PathBuf, Vec<Value>)> {
    let scheduler = task_scheduler(context).ok()?;
    let dir = scheduler.store.join("jobs");
    let jobs = scheduler
        .list()
        .ok()?
        .into_iter()
        .filter_map(|job| serde_json::to_value(job).ok())
        .collect();
    Some((dir, jobs))
}

pub(super) fn jobs_picker(context: &SlashContext<'_>) -> PickerSpec {
    let Some((dir, jobs)) = tracked_jobs(context) else {
        return PickerSpec::new(
            "Background jobs",
            vec![
                PickerItem::action("No Rust session tracks background jobs", "")
                    .detail("Start a session and run `/tan <work>` manually.")
                    .badge("empty")
                    .disabled(true),
            ],
        );
    };
    let items = if jobs.is_empty() {
        vec![PickerItem::action("No tracked background jobs", "")
            .detail(format!("Job metadata directory: {}", dir.display()))
            .badge("empty")
            .disabled(true)]
    } else {
        jobs.into_iter()
            .map(|job| {
                let id = job.get("id").and_then(Value::as_str).unwrap_or("job");
                let recorded_status = job
                    .get("status")
                    .and_then(Value::as_str)
                    .unwrap_or("not recorded");
                let task = job.get("task").and_then(Value::as_str).unwrap_or("");
                let pid = job
                    .get("pid")
                    .map(Value::to_string)
                    .unwrap_or_else(|| "not recorded".into());
                PickerItem::action(id, format!("/copy {task}"))
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
    let Some((dir, jobs)) = tracked_jobs(context) else {
        return Ok("No background jobs are tracked for a Rust session yet.".into());
    };
    if jobs.is_empty() {
        Ok(format!(
            "No background jobs are tracked in {}.",
            dir.display()
        ))
    } else {
        serde_json::to_string_pretty(&jobs).map_err(|e| e.to_string())
    }
}
