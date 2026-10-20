//! Whether a spawn request is allowed in at all: how deep it sits, what its
//! parent permits, and whether a parallel slot is free.
//!
//! Split out of `scheduler.rs` on the seam between deciding to admit work and
//! doing it, so that file fits the three-hundred-line limit the operator's
//! write guard enforces.

use super::support::process_alive;
use super::{SpawnRequest, TaskScheduler};
use crate::task_runtime::types::{AgentDefinition, JobRecord, JobStatus, TaskError};
use std::fs;
use std::path::PathBuf;

/// The depth of a task no task spawned, as the Delegated tasks section of
/// https://jeden.wisent.com/docs/tools states.
const TOP_DEPTH: u32 = 0;
/// How much deeper a delegated task sits than the task that spawned it, as
/// the Delegated tasks section of https://jeden.wisent.com/docs/tools states.
const CHILD_STEP: u32 = 1;

impl TaskScheduler {
    pub(super) fn depth_and_parent(
        &self,
        request: &SpawnRequest,
    ) -> Result<(u32, Option<JobRecord>), TaskError> {
        if let Some(id) = &request.parent_job {
            let parent = self.get(id)?;
            let depth = parent.depth.checked_add(CHILD_STEP).ok_or_else(|| {
                TaskError::Invalid(format!(
                    "job {id} sits at depth {}, the deepest a task can be counted; no task can sit below it",
                    parent.depth
                ))
            })?;
            return Ok((depth, Some(parent)));
        }
        // A task process this scheduler started is told its depth; one nobody
        // started is told none and sits at the top.
        match std::env::var("JEDEN_TASK_DEPTH") {
            Ok(written) => written
                .parse::<u32>()
                .map(|depth| (depth, None))
                .map_err(|error| {
                    TaskError::Invalid(format!(
                        "JEDEN_TASK_DEPTH is {written:?}, which is not a task depth: {error}"
                    ))
                }),
            Err(std::env::VarError::NotPresent) => Ok((TOP_DEPTH, None)),
            Err(error) => Err(TaskError::Invalid(format!(
                "JEDEN_TASK_DEPTH cannot be read: {error}"
            ))),
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
            // The parent's own rules decide what it may spawn, so a parent
            // whose definition is no longer loaded spawns nothing.
            let parent_def = agents.get(&parent.agent).ok_or_else(|| {
                TaskError::NotFound(format!(
                    "the parent job {} runs agent {}, which no loaded agent definition declares",
                    parent.id, parent.agent
                ))
            })?;
            if (parent.agent == request.agent && !parent_def.spawn.allow_recursive)
                || parent_def
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
    /// Take a free parallel slot for `job_id`. A slot is published whole: its
    /// holder's process id and job id are written to a staging file first and
    /// linked to the slot's place among the `maxParallel` slots, so no reader
    /// ever sees a slot without its holder; a place already linked is taken.
    pub(super) fn reserve_slot(&self, job_id: &str) -> Result<PathBuf, TaskError> {
        self.clean_slots()?;
        let staging = self.store.join(format!("slot-{job_id}.writing"));
        fs::write(&staging, format!("{}\n{}\n", std::process::id(), job_id)).map_err(|error| {
            TaskError::Io(format!(
                "cannot write the admission slot of job {job_id} at {}: {error}",
                staging.display()
            ))
        })?;
        let mut places = std::iter::repeat(())
            .take(self.limits.max_parallel)
            .enumerate();
        let reserved = places.find_map(|(place, ())| {
            let path = self.store.join("slots").join(place.to_string());
            match fs::hard_link(&staging, &path) {
                Ok(()) => Some(Ok(path)),
                Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => None,
                Err(error) => Some(Err(TaskError::Io(format!(
                    "cannot take admission slot {}: {error}",
                    path.display()
                )))),
            }
        });
        if let Err(error) = fs::remove_file(&staging) {
            let unstaged = format!(
                "cannot remove the admission slot staging file {}: {error}",
                staging.display()
            );
            return Err(TaskError::Io(match reserved {
                Some(Ok(path)) => match fs::remove_file(&path) {
                    Ok(()) => format!("{unstaged}; slot {} was given back", path.display()),
                    Err(release) => format!(
                        "{unstaged}; slot {} could not be given back either: {release}",
                        path.display()
                    ),
                },
                _ => unstaged,
            }));
        }
        match reserved {
            Some(taken) => taken,
            None => {
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
        }
    }
    pub(super) fn clean_slots(&self) -> Result<(), TaskError> {
        for entry in fs::read_dir(self.store.join("slots"))? {
            let path = entry?.path();
            // A slot that cannot be read names a holder nobody can check;
            // freed as dead, it admitted one task more than maxParallel.
            let text = match fs::read_to_string(&path) {
                Ok(text) => text,
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
                Err(error) => {
                    return Err(std::io::Error::new(
                        error.kind(),
                        format!("cannot read admission slot {}: {error}", path.display()),
                    )
                    .into())
                }
            };
            // Slots are published whole, so a slot that names no process id
            // as its holder is damaged, not free.
            let pid = match text.lines().next() {
                Some(holder) => holder.parse::<u32>().map_err(|error| {
                    TaskError::Invalid(format!(
                        "admission slot {} is damaged: its holder {holder:?} is no process id: {error}",
                        path.display()
                    ))
                })?,
                None => {
                    return Err(TaskError::Invalid(format!(
                        "admission slot {} is damaged: it names no holder",
                        path.display()
                    )))
                }
            };
            if !process_alive(pid) {
                // A dead holder's slot that cannot be removed keeps counting
                // against maxParallel, so the failure is the answer; one
                // another cleaner already freed is gone either way.
                match fs::remove_file(&path) {
                    Ok(()) => {}
                    Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                    Err(error) => {
                        return Err(std::io::Error::new(
                            error.kind(),
                            format!("cannot free admission slot {}: {error}", path.display()),
                        )
                        .into())
                    }
                }
            }
        }
        Ok(())
    }
}
