use super::discovery::discover_agents;
use super::mailbox::Mailbox;
use super::types::{AgentDefinition, JobRecord, JobStatus, TaskError, TaskLimits};
use super::{atomic_json, ensure_store_schema};
use crate::clock::unix_millis;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

mod admission;
mod lifecycle;
mod spawn;
mod support;

use support::process_alive;
pub(crate) use support::workspace_root_for;

#[cfg(unix)]
unsafe extern "C" {
    fn kill(pid: i32, signal: i32) -> i32;
}

#[derive(Clone, Debug)]
pub struct TaskScheduler {
    pub store: PathBuf,
    pub cwd: PathBuf,
    pub limits: TaskLimits,
    exe: PathBuf,
    /// Where isolated job workspaces are made, resolved once when the
    /// scheduler opens.
    workspace_root: PathBuf,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SpawnRequest {
    pub task: String,
    #[serde(default = "default_agent")]
    pub agent: String,
    #[serde(default)]
    pub model: Option<String>,
    /// The child run's step bound when the caller states one; unstated, the
    /// child runs until its work ends, as `jeden run` does.
    #[serde(default)]
    pub max_steps: Option<u64>,
    #[serde(default)]
    pub parent_job: Option<String>,
    #[serde(default)]
    pub isolate: Option<bool>,
}
fn default_agent() -> String {
    "default".into()
}
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BatchTask {
    pub id: String,
    pub task: String,
    #[serde(default = "default_agent")]
    pub agent: String,
    #[serde(default)]
    pub depends_on: Vec<String>,
}

impl TaskScheduler {
    /// Opens the store under the operator's declared `limits`, used as they
    /// were declared: `limits_from_config` already refused a missing or
    /// non-positive one by name.
    pub fn open(cwd: &Path, store: &Path, limits: TaskLimits) -> Result<Self, TaskError> {
        for path in [
            store.to_path_buf(),
            store.join("jobs"),
            store.join("workspaces"),
            store.join("slots"),
            store.join("sessions"),
        ] {
            fs::create_dir_all(path)?;
        }
        ensure_store_schema(store)?;
        // The executable a job runs: the one JEDEN_TASK_EXECUTABLE names,
        // else this running jeden.
        let exe = match std::env::var_os("JEDEN_TASK_EXECUTABLE") {
            Some(path) => PathBuf::from(path),
            None => {
                std::env::current_exe().map_err(|error| TaskError::Process(error.to_string()))?
            }
        };
        let this = Self {
            store: store.into(),
            cwd: cwd.into(),
            limits,
            exe,
            workspace_root: workspace_root_for(store, cwd)?,
        };
        this.recover()?;
        Ok(this)
    }
    pub fn mailbox(&self) -> Result<Mailbox, TaskError> {
        Mailbox::new(&self.store)
    }
    fn workspace_root(&self) -> PathBuf {
        self.workspace_root.clone()
    }
    fn job_path(&self, id: &str) -> PathBuf {
        self.store.join("jobs").join(format!("{id}.json"))
    }
    fn write_job(&self, job: &JobRecord) -> Result<(), TaskError> {
        atomic_json(&self.job_path(&job.id), job)
    }
    pub fn get(&self, id: &str) -> Result<JobRecord, TaskError> {
        let path = self.job_path(id);
        if !crate::tool_runtime::runtime_ops::platform::presence::present(&path)
            .map_err(TaskError::Invalid)?
        {
            return Err(TaskError::NotFound(format!("job not found: {id}")));
        }
        Ok(serde_json::from_slice(&fs::read(path)?)?)
    }
    /// Every job record. An entry or record that cannot be read or parsed
    /// is refused naming its file: dropped, a running job was never
    /// recovered and never listed.
    pub fn list(&self) -> Result<Vec<JobRecord>, TaskError> {
        let mut jobs = Vec::new();
        let directory = self.store.join("jobs");
        let unlistable = |error: std::io::Error| {
            TaskError::Io(format!(
                "job records in {} cannot be listed: {error}",
                directory.display()
            ))
        };
        for entry in fs::read_dir(&directory).map_err(unlistable)? {
            let path = entry.map_err(unlistable)?.path();
            if path.extension().and_then(|v| v.to_str()) == Some("json") {
                let bytes = fs::read(&path).map_err(|error| {
                    TaskError::Io(format!(
                        "job record {} cannot be read: {error}",
                        path.display()
                    ))
                })?;
                let job = serde_json::from_slice::<JobRecord>(&bytes).map_err(|error| {
                    TaskError::Invalid(format!(
                        "job record {} is not a job: {error}",
                        path.display()
                    ))
                })?;
                jobs.push(job);
            }
        }
        jobs.sort_by(|a, b| {
            a.created_at
                .cmp(&b.created_at)
                .then_with(|| a.id.cmp(&b.id))
        });
        Ok(jobs)
    }
    fn agents(&self) -> Result<BTreeMap<String, AgentDefinition>, TaskError> {
        Ok(discover_agents(&self.cwd)?
            .into_iter()
            .map(|a| (a.id.clone(), a))
            .collect())
    }
    fn recover(&self) -> Result<(), TaskError> {
        let now = unix_millis()?;
        for mut job in self.list()? {
            if job.status == JobStatus::Running
                && !matches!(job.pid, Some(pid) if process_alive(pid))
            {
                job.status = JobStatus::Interrupted;
                job.pid = None;
                job.updated_at = now;
                job.error = Some(
                    "scheduler recovered job after its process exited without a terminal record"
                        .into(),
                );
                self.write_job(&job)?;
            }
        }
        self.clean_slots()
    }
}
