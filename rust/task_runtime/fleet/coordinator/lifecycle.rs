//! What a worker reports while it is holding a job, and what the coordinator
//! does with each report.
//!
//! Split out of `task_runtime/fleet/coordinator/mod.rs`, which had grown past
//! the module line cap.

use super::{current_job_mut, Coordinator};
use super::store::JobState;
use crate::task_runtime::fleet::protocol::{
    CommitRequest, ProtocolError, WorkerEvent,
};
use crate::task_runtime::fleet::protocol::AttemptPhase;
use crate::task_runtime::fleet::protocol::JobOutcome;
use crate::task_runtime::fleet::protocol::JobPhase;

impl Coordinator {
    pub fn acknowledge(
        &self,
        worker_id: &str,
        job_id: &str,
        attempt: u32,
        fence: u64,
        now: u64,
    ) -> Result<(), ProtocolError> {
        self.store.transact(|state| {
            let job = current_job_mut(state.jobs.get_mut(job_id), worker_id, attempt, fence, now)?;
            job.phase = JobPhase::Running;
            let current = job
                .attempts
                .last_mut()
                .ok_or_else(|| ProtocolError::Conflict("attempt record missing".into()))?;
            current.phase = AttemptPhase::Accepted;
            current.updated_at = now;
            Ok(())
        })
    }
    pub fn heartbeat(
        &self,
        worker_id: &str,
        job_id: &str,
        attempt: u32,
        fence: u64,
        now: u64,
    ) -> Result<u64, ProtocolError> {
        self.store.transact(|state| {
            let worker = state.workers.get_mut(worker_id).ok_or_else(|| {
                ProtocolError::NotFound(format!("worker not registered: {worker_id}"))
            })?;
            worker.last_heartbeat = now;
            let job = current_job_mut(state.jobs.get_mut(job_id), worker_id, attempt, fence, now)?;
            let lease = job.lease.as_mut().expect("validated lease");
            lease.heartbeat_at = now;
            lease.expires_at = now.saturating_add(self.lease_ms);
            Ok(lease.expires_at)
        })
    }
    pub fn record_event(
        &self,
        worker_id: &str,
        event: WorkerEvent,
        now: u64,
    ) -> Result<bool, ProtocolError> {
        self.store.transact(|state| {
            let job = current_job_mut(
                state.jobs.get_mut(&event.job_id),
                worker_id,
                event.attempt,
                event.fencing_token,
                now,
            )?;
            let key = (event.attempt, event.sequence);
            if let Some(existing) = job.events.get(&key) {
                if existing == &event {
                    return Ok(false);
                }
                return Err(ProtocolError::Conflict(format!(
                    "event sequence {} was reused with different content",
                    event.sequence
                )));
            }
            let next = job
                .events
                .keys()
                .filter(|(attempt, _)| *attempt == event.attempt)
                .map(|(_, sequence)| *sequence)
                .max()
                .unwrap_or(0)
                .saturating_add(1);
            if event.sequence != next {
                return Err(ProtocolError::Conflict(format!(
                    "event sequence gap: expected {next}, got {}",
                    event.sequence
                )));
            }
            if let Some(current) = job.attempts.last_mut() {
                current.phase = event.phase;
                current.updated_at = now;
            }
            job.events.insert(key, event);
            Ok(true)
        })
    }
    pub fn replay_events(
        &self,
        job_id: &str,
        attempt: u32,
        after_sequence: u64,
    ) -> Result<Vec<WorkerEvent>, ProtocolError> {
        let job = self.store.job(job_id)?;
        Ok(job
            .events
            .into_iter()
            .filter(|((event_attempt, sequence), _)| {
                *event_attempt == attempt && *sequence > after_sequence
            })
            .map(|(_, event)| event)
            .collect())
    }
    pub fn commit(
        &self,
        worker_id: &str,
        request: CommitRequest,
        now: u64,
    ) -> Result<JobOutcome, ProtocolError> {
        if !self
            .cas
            .contains(request.output_root)
            .map_err(|e| ProtocolError::Storage(e.to_string()))?
        {
            return Err(ProtocolError::Storage(format!(
                "output root {} is absent from CAS",
                request.output_root
            )));
        }
        self.store.transact(|state| {
            let job = current_job_mut(
                state.jobs.get_mut(&request.job_id),
                worker_id,
                request.attempt,
                request.fencing_token,
                now,
            )?;
            if job.phase == JobPhase::Cancelling {
                return Err(ProtocolError::Cancelled(format!(
                    "job {} is cancelling",
                    request.job_id
                )));
            }
            if job.phase.terminal() {
                return Err(ProtocolError::Conflict(format!(
                    "job {} is already terminal",
                    request.job_id
                )));
            }
            let output = JobOutcome {
                job_id: request.job_id.clone(),
                attempt: request.attempt,
                output_root: request.output_root,
                result: request.result.clone(),
            };
            job.phase = JobPhase::Succeeded;
            job.outcome = Some(output.clone());
            job.lease = None;
            if let Some(attempt) = job.attempts.last_mut() {
                attempt.phase = AttemptPhase::Succeeded;
                attempt.updated_at = now;
            }
            if let Some(worker) = state.workers.get_mut(worker_id) {
                worker.running = worker.running.saturating_sub(1);
            }
            Ok(output)
        })
    }
}
