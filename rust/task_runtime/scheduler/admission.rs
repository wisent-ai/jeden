//! Whether a spawn request is allowed in at all: how deep it sits, what its
//! parent permits, and whether a parallel slot is free.
//!
//! Split out of `scheduler.rs` on the seam between deciding to admit work and
//! doing it, so that file fits the three-hundred-line limit the operator's
//! write guard enforces.

use super::support::process_alive;
use super::{SpawnRequest, TaskScheduler};
use crate::task_runtime::types::{AgentDefinition, JobRecord, JobStatus, TaskError};
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::PathBuf;

impl TaskScheduler {
    pub(super) fn depth_and_parent(
        &self,
        request: &SpawnRequest,
    ) -> Result<(u32, Option<JobRecord>), TaskError> {
        if let Some(id) = &request.parent_job {
            let parent = self.get(id)?;
            Ok((parent.depth.saturating_add(1), Some(parent)))
        } else {
            Ok((
                std::env::var("JEDEN_TASK_DEPTH")
                    .ok()
                    .and_then(|v| v.parse().ok())
                    .unwrap_or(0),
                None,
            ))
        }
    }
    pub(super) fn validate_policy(
        &self,
        request: &SpawnRequest,
        definition: &AgentDefinition,
        depth: u32,
        parent: Option<&JobRecord>,
    ) -> Result<(), TaskError> {
        if depth > self.limits.max_depth {
            return Err(TaskError::RecursionDenied {
                agent: request.agent.clone(),
                depth,
            });
        }
        if definition
            .spawn
            .deny_agents
            .iter()
            .any(|v| v == &request.agent)
        {
            return Err(TaskError::RecursionDenied {
                agent: request.agent.clone(),
                depth,
            });
        }
        if let Some(parent) = parent {
            let agents = self.agents()?;
            let parent_def = agents.get(&parent.agent);
            if parent.agent == request.agent
                && !parent_def.map(|d| d.spawn.allow_recursive).unwrap_or(false)
            {
                return Err(TaskError::RecursionDenied {
                    agent: request.agent.clone(),
                    depth,
                });
            }
            if let Some(parent_def) = parent_def {
                if parent_def
                    .spawn
                    .deny_agents
                    .iter()
                    .any(|v| v == &request.agent)
                    || (!parent_def.spawn.allow_agents.is_empty()
                        && !parent_def
                            .spawn
                            .allow_agents
                            .iter()
                            .any(|v| v == &request.agent))
                {
                    return Err(TaskError::RecursionDenied {
                        agent: request.agent.clone(),
                        depth,
                    });
                }
            }
            let children = self
                .list()?
                .into_iter()
                .filter(|j| j.parent_job.as_deref() == Some(&parent.id))
                .count();
            if children >= self.limits.max_children {
                return Err(TaskError::Capacity {
                    running: children,
                    limit: self.limits.max_children,
                });
            }
        }
        Ok(())
    }
    pub(super) fn reserve_slot(&self, job_id: &str) -> Result<PathBuf, TaskError> {
        self.clean_slots()?;
        for index in 0..self.limits.max_parallel.max(1) {
            let path = self.store.join("slots").join(index.to_string());
            match OpenOptions::new().create_new(true).write(true).open(&path) {
                Ok(mut file) => {
                    write!(file, "{}\n{}\n", std::process::id(), job_id)?;
                    return Ok(path);
                }
                Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {}
                Err(error) => return Err(error.into()),
            }
        }
        let running = self
            .list()?
            .into_iter()
            .filter(|j| j.status == JobStatus::Running)
            .count();
        Err(TaskError::Capacity {
            running,
            limit: self.limits.max_parallel,
        })
    }
    pub(super) fn clean_slots(&self) -> Result<(), TaskError> {
        for entry in fs::read_dir(self.store.join("slots"))?.flatten() {
            let path = entry.path();
            let pid = fs::read_to_string(&path)
                .ok()
                .and_then(|v| v.lines().next()?.parse::<u32>().ok());
            if !pid.map(process_alive).unwrap_or(false) {
                let _ = fs::remove_file(path);
            }
        }
        Ok(())
    }
}
