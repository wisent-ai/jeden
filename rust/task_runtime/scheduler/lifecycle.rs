//! What happens to a job after it starts: waiting on it, marking it
//! delivered, cancelling it and its descendants, merging its captured patch
//! back, running a dependency-ordered batch, and reporting capability health.
//!
//! Split out of `scheduler.rs` so that file fits the three-hundred-line limit
//! the operator's write guard enforces. That split is what made the isolation
//! strategy list in `health` correctable at all.

use super::support::terminate_group;
use super::{default_steps, BatchTask, SpawnRequest, TaskScheduler};
use crate::task_runtime::discovery::discover_agents;
use crate::task_runtime::now_millis;
use crate::task_runtime::types::{CapabilityHealth, JobRecord, JobStatus, TaskError};
use crate::task_runtime::workspace::IsolatedWorkspace;
use serde_json::json;
use std::collections::BTreeSet;
use std::thread;
use std::time::{Duration, Instant};

/// How often a waiting caller re-reads a job record. Short enough that a
/// finished job is reported promptly, long enough not to spin a core.
const POLL_INTERVAL: Duration = Duration::from_millis(50);

impl TaskScheduler {
    pub fn poll(&self, id: &str, wait: Duration) -> Result<JobRecord, TaskError> {
        let deadline = Instant::now() + wait;
        loop {
            let job = self.get(id)?;
            if job.status.terminal() || Instant::now() >= deadline {
                return Ok(job);
            }
            thread::sleep(POLL_INTERVAL);
        }
    }
    pub fn deliver(&self, id: &str) -> Result<JobRecord, TaskError> {
        let mut job = self.get(id)?;
        if !job.status.terminal() {
            return Err(TaskError::Conflict(format!("job {id} is not terminal")));
        }
        job.delivered = true;
        job.updated_at = now_millis();
        self.write_job(&job)?;
        Ok(job)
    }
    pub fn cancel(&self, id: &str) -> Result<Vec<String>, TaskError> {
        let jobs = self.list()?;
        let mut targets = BTreeSet::from([id.to_string()]);
        loop {
            let before = targets.len();
            for job in &jobs {
                if job
                    .parent_job
                    .as_ref()
                    .map(|v| targets.contains(v))
                    .unwrap_or(false)
                {
                    targets.insert(job.id.clone());
                }
            }
            if targets.len() == before {
                break;
            }
        }
        let mut cancelled = Vec::new();
        for target in targets.iter().rev() {
            let mut job = self.get(target)?;
            if !job.status.terminal() {
                if let Some(pid) = job.pid {
                    terminate_group(pid, self.limits.kill_grace_ms);
                }
                job.status = JobStatus::Cancelled;
                job.pid = None;
                job.updated_at = now_millis();
                self.write_job(&job)?;
                cancelled.push(target.clone());
            }
        }
        Ok(cancelled)
    }
    pub fn merge(&self, id: &str) -> Result<JobRecord, TaskError> {
        let mut job = self.get(id)?;
        if !job.status.terminal() {
            return Err(TaskError::Conflict("cannot merge a running job".into()));
        }
        IsolatedWorkspace {
            path: job.workspace.clone(),
            strategy: job.isolation.clone(),
            parent: job.cwd.clone(),
        }
        .merge(&job.capture)?;
        job.metadata.insert("mergedAt".into(), json!(now_millis()));
        job.updated_at = now_millis();
        self.write_job(&job)?;
        Ok(job)
    }
    pub fn batch(&self, tasks: Vec<BatchTask>) -> Result<Vec<JobRecord>, TaskError> {
        if tasks.is_empty() || tasks.len() > self.limits.max_batch {
            return Err(TaskError::Invalid(format!(
                "batch size must be 1..{}",
                self.limits.max_batch
            )));
        }
        let ids = tasks.iter().map(|t| t.id.clone()).collect::<BTreeSet<_>>();
        if ids.len() != tasks.len() {
            return Err(TaskError::Invalid("batch task ids must be unique".into()));
        }
        if tasks
            .iter()
            .flat_map(|t| &t.depends_on)
            .any(|d| !ids.contains(d))
        {
            return Err(TaskError::Invalid("batch dependency is missing".into()));
        }
        let mut completed = BTreeSet::new();
        let mut results = Vec::new();
        while completed.len() < tasks.len() {
            let ready = tasks
                .iter()
                .filter(|t| {
                    !completed.contains(&t.id) && t.depends_on.iter().all(|d| completed.contains(d))
                })
                .take(self.limits.max_parallel)
                .cloned()
                .collect::<Vec<_>>();
            if ready.is_empty() {
                return Err(TaskError::Invalid("batch DAG contains a cycle".into()));
            }
            let mut wave = Vec::new();
            for task in ready {
                let job = self.spawn(SpawnRequest {
                    task: task.task,
                    agent: task.agent,
                    model: None,
                    max_steps: default_steps(),
                    parent_job: None,
                    isolate: Some(true),
                })?;
                wave.push((task.id, job.id));
            }
            for (task_id, job_id) in wave {
                let job = self.poll(&job_id, Duration::from_millis(self.limits.wait_budget_ms))?;
                if !job.status.terminal() {
                    self.cancel(&job_id)?;
                    return Err(TaskError::Timeout(format!(
                        "batch task exceeded its wait budget: {task_id}"
                    )));
                }
                completed.insert(task_id);
                results.push(job);
            }
        }
        Ok(results)
    }
    pub fn health(&self) -> CapabilityHealth {
        let agents = discover_agents(&self.cwd);
        let jobs = self.list();
        let mut errors = Vec::new();
        if let Err(e) = &agents {
            errors.push(e.to_string());
        }
        if let Err(e) = &jobs {
            errors.push(e.to_string());
        }
        CapabilityHealth {
            id: "task-scheduler",
            healthy: errors.is_empty(),
            store: self.store.clone(),
            discovered_agents: agents.as_ref().map(|v| v.len()).unwrap_or(0),
            running: jobs
                .as_ref()
                .map(|v| v.iter().filter(|j| j.status == JobStatus::Running).count())
                .unwrap_or(0),
            limits: self.limits.clone(),
            // Exactly the values `WorkspacePlatform::isolate` can return, read
            // from `platform/unix/workspace.rs` (`apfs-clone`, `reflink-copy`,
            // `native-copy`) and `platform/windows/workspace.rs`
            // (`native-copy`). The list previously advertised `git-worktree`,
            // which this product no longer does, and `copy`, which no
            // implementation ever returned.
            isolation_strategies: vec!["apfs-clone", "reflink-copy", "native-copy"],
            errors,
        }
    }
}
