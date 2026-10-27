//! Starting one job: isolate a workspace, build the sandboxed child command,
//! launch it, and hand it to a reaper thread that records how it ended.
//!
//! Split out of `scheduler.rs` so that file fits the three-hundred-line limit
//! the operator's write guard enforces. The isolated workspace here is a copy
//! or a filesystem clone — never a Git worktree; see
//! `platform/unix/workspace.rs`, from which that branch was removed.

use super::support::{
    agent_task_context, capture_pipe, configure_group, definition_output_is_unconstrained,
    validate_output,
};
use super::{SpawnRequest, TaskScheduler};
use crate::clock::unix_millis;
use crate::task_runtime::types::{JobRecord, JobStatus, TaskError};
use crate::task_runtime::workspace::{isolate, IsolatedWorkspace};
use serde_json::json;
use std::collections::BTreeMap;
use std::fs;
use std::process::Stdio;
use std::thread;

impl TaskScheduler {
    pub fn spawn(&self, request: SpawnRequest) -> Result<JobRecord, TaskError> {
        if request.task.trim().is_empty() {
            return Err(TaskError::Invalid("task text is required".into()));
        }
        let agents = self.agents()?;
        let definition = agents
            .get(&request.agent)
            .ok_or_else(|| TaskError::NotFound(format!("agent not found: {}", request.agent)))?;
        let (depth, parent) = self.depth_and_parent(&request)?;
        self.validate_policy(&request, definition, depth, parent.as_ref())?;
        let child_task = agent_task_context(&self.cwd, definition, &request.task)?;
        let created = unix_millis()?;
        let id = format!("job-{created}-{}", uuid::Uuid::new_v4().simple());
        let slot = self.reserve_slot(&id)?;
        let isolated = match isolate(&self.cwd, &self.workspace_root(), &id) {
            Ok(v) => v,
            Err(error) => return Err(released(&slot, error)),
        };
        let stdout = self.store.join("jobs").join(format!("{id}.stdout.log"));
        let stderr = self.store.join("jobs").join(format!("{id}.stderr.log"));
        let capture = self.store.join("jobs").join(format!("{id}.patch"));
        let session_path = self.store.join("sessions").join(&id);
        if let Err(error) = fs::create_dir_all(&session_path) {
            return Err(released(&slot, error.into()));
        }
        let mut command = crate::task_runtime::sandbox::command(
            &self.exe,
            &[
                self.cwd.clone(),
                isolated.path.clone(),
                session_path.clone(),
                self.store.clone(),
            ],
            &[
                isolated.path.clone(),
                session_path.clone(),
                self.store.clone(),
            ],
        )
        .map_err(|error| released(&slot, TaskError::Process(error)))?;
        command
            .arg("run")
            .arg(&child_task)
            .arg("--cwd")
            .arg(&isolated.path)
            .arg("--json")
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        if let Some(max_steps) = request.max_steps {
            command.arg("--max-steps").arg(max_steps.to_string());
        }
        if let Some(model) = request.model.as_ref().or(definition.model.as_ref()) {
            command.arg("--model").arg(model);
        }
        command
            .env("JEDEN_TASK_JOB", &id)
            .env("JEDEN_TASK_DEPTH", depth.to_string())
            .env("JEDEN_TASK_SESSION", &session_path)
            .env("JEDEN_SESSION_ROOT", &session_path);
        if !definition.tools.is_empty() {
            command.env("JEDEN_AGENT_TOOLS", definition.tools.join(","));
        }
        configure_group(&mut command);
        let mut child = match command.spawn() {
            Ok(child) => child,
            Err(error) => return Err(released(&slot, TaskError::Process(error.to_string()))),
        };
        let child_stdout = child
            .stdout
            .take()
            .ok_or_else(|| TaskError::Process("child stdout pipe unavailable".into()))?;
        let child_stderr = child
            .stderr
            .take()
            .ok_or_else(|| TaskError::Process("child stderr pipe unavailable".into()))?;
        fs::write(&slot, format!("{}\n{}\n", child.id(), id))?;
        let job = JobRecord {
            id: id.clone(),
            task: request.task,
            agent: request.agent,
            status: JobStatus::Running,
            cwd: self.cwd.clone(),
            workspace: isolated.path.clone(),
            isolation: isolated.strategy.clone(),
            session_path,
            stdout,
            stderr,
            capture,
            pid: Some(child.id()),
            parent_job: request.parent_job,
            depth,
            created_at: created,
            updated_at: created,
            exit_code: None,
            error: None,
            delivered: false,
            metadata: BTreeMap::from([
                ("definition".into(), json!(definition.source)),
                ("tools".into(), json!(definition.tools)),
                ("skills".into(), json!(definition.skills)),
                ("outputSchema".into(), definition.output.clone()),
            ]),
        };
        self.write_job(&job)?;
        let scheduler = self.clone();
        let mut reaper_job = job.clone();
        let stdout_path = job.stdout.clone();
        let stderr_path = job.stderr.clone();
        let max_output = self.limits.max_output_bytes;
        thread::Builder::new()
            .name(format!("task-reaper-{id}"))
            .spawn(move || {
                let (status, stdout_result, stderr_result) = thread::scope(|scope| {
                    let stdout_reader =
                        scope.spawn(|| capture_pipe(child_stdout, &stdout_path, max_output));
                    let stderr_reader =
                        scope.spawn(|| capture_pipe(child_stderr, &stderr_path, max_output));
                    let status = stado_wait::blocking(
                        stado_wait::Kind::Process,
                        format!("task job {}", reaper_job.id),
                        reaper_job.workspace.display(),
                        || child.wait(),
                    );
                    let stdout_result = match stdout_reader.join() {
                        Ok(result) => result,
                        Err(panic) => Err(TaskError::Process(crate::panicked::refusal(
                            "stdout capture",
                            &*panic,
                        ))),
                    };
                    let stderr_result = match stderr_reader.join() {
                        Ok(result) => result,
                        Err(panic) => Err(TaskError::Process(crate::panicked::refusal(
                            "stderr capture",
                            &*panic,
                        ))),
                    };
                    (status, stdout_result, stderr_result)
                });
                // A clock that cannot be read leaves the record at its start
                // time and says so beside whatever else the job reports.
                let ended = unix_millis();
                if let Ok(ended) = ended {
                    reaper_job.updated_at = ended;
                }
                reaper_job.pid = None;
                if let Err(error) = stdout_result.and(stderr_result) {
                    reaper_job.error = Some(error.to_string());
                }
                // An unreadable record is said last, below, and read as not cancelled.
                let current = scheduler.get(&reaper_job.id);
                let cancelled =
                    matches!(&current, Ok(current) if current.status == JobStatus::Cancelled);
                if cancelled {
                    reaper_job.status = JobStatus::Cancelled;
                } else {
                    match status {
                        Ok(status) => {
                            reaper_job.exit_code = status.code();
                            reaper_job.status = if status.success() {
                                JobStatus::Succeeded
                            } else {
                                JobStatus::Failed
                            };
                        }
                        Err(error) => {
                            reaper_job.status = JobStatus::Interrupted;
                            reaper_job.error = Some(error.to_string());
                        }
                    }
                }
                if reaper_job.status == JobStatus::Succeeded
                    && !definition_output_is_unconstrained(&reaper_job.metadata["outputSchema"])
                {
                    // A stdout that cannot be read is not "not one JSON
                    // value"; each failure says which it was.
                    let output = fs::read_to_string(&reaper_job.stdout)
                        .map_err(|error| {
                            format!(
                                "child stdout {} cannot be read: {error}",
                                reaper_job.stdout.display()
                            )
                        })
                        .and_then(|text| {
                            serde_json::from_str::<serde_json::Value>(text.trim()).map_err(
                                |error| format!("child stdout is not one JSON value: {error}"),
                            )
                        });
                    match output {
                        Ok(value) => {
                            if let Err(error) =
                                validate_output(&value, &reaper_job.metadata["outputSchema"], "$")
                            {
                                reaper_job.status = JobStatus::Failed;
                                reaper_job.error =
                                    Some(format!("agent output contract failed: {error}"));
                            }
                        }
                        Err(cause) => {
                            reaper_job.status = JobStatus::Failed;
                            reaper_job.error =
                                Some(format!("agent output contract failed: {cause}"));
                        }
                    }
                }
                let workspace = IsolatedWorkspace {
                    path: reaper_job.workspace.clone(),
                    strategy: reaper_job.isolation.clone(),
                    parent: reaper_job.cwd.clone(),
                };
                if let Err(error) = workspace.capture(&reaper_job.capture, max_output) {
                    reaper_job.error = Some(error.to_string());
                    if reaper_job.status == JobStatus::Succeeded {
                        reaper_job.status = JobStatus::Failed;
                    }
                }
                // Said last, so no later failure of the job writes over them.
                let unread = current.err().map(|error| {
                    format!("the job record could not be read back to learn whether it was cancelled: {error}")
                });
                let unclocked = ended.err().map(|error| format!("the end time was not read: {error}"));
                for cause in unread.into_iter().chain(unclocked) {
                    reaper_job.error = Some(match reaper_job.error.take() {
                        Some(earlier) => format!("{earlier}; {cause}"),
                        None => cause,
                    });
                }
                // A job whose end cannot be written stays 'running' in its
                // record for every reader; the failure is said where it
                // happens, and the slot is freed either way.
                if let Err(error) = scheduler.write_job(&reaper_job) {
                    eprintln!(
                        "jeden: job {} ended but its record cannot be written: {error}",
                        reaper_job.id
                    );
                }
                if let Err(error) = fs::remove_file(slot) {
                    if error.kind() != std::io::ErrorKind::NotFound {
                        eprintln!(
                            "jeden: the scheduler slot of job {} cannot be released: {error}",
                            reaper_job.id
                        );
                    }
                }
            })
            .map_err(|e| TaskError::Process(e.to_string()))?;
        Ok(job)
    }
}

/// `error`, after the scheduler slot reserved for a job that never started
/// is released; a slot that cannot be released is said, since it holds a
/// place in the running limit until it is removed.
fn released(slot: &std::path::Path, error: TaskError) -> TaskError {
    if let Err(left) = fs::remove_file(slot) {
        if left.kind() != std::io::ErrorKind::NotFound {
            eprintln!(
                "jeden: the scheduler slot {} of a job that did not start cannot be released: {left}",
                slot.display()
            );
        }
    }
    error
}
