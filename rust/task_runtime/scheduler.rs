use super::discovery::discover_agents;
use super::mailbox::Mailbox;
use super::types::{AgentDefinition, JobRecord, JobStatus, TaskError, TaskLimits};
use super::{atomic_json, ensure_store_schema, now_millis};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicU64;

mod admission;
mod lifecycle;
mod spawn;
mod support;

use support::process_alive;
pub(crate) use support::workspace_root_for;

static JOB_SEQUENCE: AtomicU64 = AtomicU64::new(1);

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
}

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SpawnRequest {
    pub task: String,
    #[serde(default = "default_agent")]
    pub agent: String,
    #[serde(default)]
    pub model: Option<String>,
    #[serde(default = "default_steps")]
    pub max_steps: u64,
    #[serde(default)]
    pub parent_job: Option<String>,
    #[serde(default)]
    pub isolate: Option<bool>,
}
fn default_agent() -> String {
    "default".into()
}
fn default_steps() -> u64 {
    6
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
    pub fn open(cwd: &Path, store: &Path, mut limits: TaskLimits) -> Result<Self, TaskError> {
        limits.max_parallel = limits.max_parallel.clamp(1, 32);
        limits.max_batch = limits.max_batch.clamp(1, 256);
        limits.max_depth = limits.max_depth.min(16);
        limits.max_children = limits.max_children.clamp(1, 256);
        limits.max_output_bytes = limits.max_output_bytes.clamp(1_024, 64 * 1024 * 1024);
        limits.wait_budget_ms = limits.wait_budget_ms.clamp(100, 3_600_000);
        limits.kill_grace_ms = limits.kill_grace_ms.clamp(10, 30_000);
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
        let exe = std::env::var_os("JEDEN_TASK_EXECUTABLE")
            .map(PathBuf::from)
            .map(Ok)
            .unwrap_or_else(|| {
                std::env::current_exe().map_err(|error| TaskError::Process(error.to_string()))
            })?;
        let this = Self {
            store: store.into(),
            cwd: cwd.into(),
            limits,
            exe,
        };
        this.recover()?;
        Ok(this)
    }
    pub fn mailbox(&self) -> Result<Mailbox, TaskError> {
        Mailbox::new(&self.store, self.limits.max_children.saturating_mul(64))
    }
    fn workspace_root(&self) -> PathBuf {
        workspace_root_for(&self.store, &self.cwd)
    }
    fn job_path(&self, id: &str) -> PathBuf {
        self.store.join("jobs").join(format!("{id}.json"))
    }
    fn write_job(&self, job: &JobRecord) -> Result<(), TaskError> {
        atomic_json(&self.job_path(&job.id), job)
    }
    pub fn get(&self, id: &str) -> Result<JobRecord, TaskError> {
        let path = self.job_path(id);
        if !path.exists() {
            return Err(TaskError::NotFound(format!("job not found: {id}")));
        }
        Ok(serde_json::from_slice(&fs::read(path)?)?)
    }
    pub fn list(&self) -> Result<Vec<JobRecord>, TaskError> {
        let mut jobs = Vec::new();
        for entry in fs::read_dir(self.store.join("jobs"))?.flatten() {
            if entry.path().extension().and_then(|v| v.to_str()) == Some("json") {
                if let Ok(job) = serde_json::from_slice::<JobRecord>(&fs::read(entry.path())?) {
                    jobs.push(job);
                }
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
        for mut job in self.list()? {
            if job.status == JobStatus::Running && !job.pid.map(process_alive).unwrap_or(false) {
                job.status = JobStatus::Interrupted;
                job.pid = None;
                job.updated_at = now_millis();
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
