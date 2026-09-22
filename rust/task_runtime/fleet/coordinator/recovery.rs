//! What happens when a job has to stop, or a worker stops answering.
//!
//! Split out of `task_runtime/fleet/coordinator/mod.rs`, which had grown past
//! the module line cap.

use super::{current_job_mut_allow_cancelling, Coordinator};
use crate::task_runtime::fleet::protocol::ProtocolError;
use crate::task_runtime::fleet::coordinator::current_job_mut;
use crate::task_runtime::fleet::protocol::AttemptPhase;
use crate::task_runtime::fleet::protocol::JobPhase;
use crate::task_runtime::fleet::protocol::WorkOffer;

impl Coordinator {
    pub fn cancel(&self, job_id: &str, now: u64) -> Result<bool, ProtocolError> {
        self.store.transact(|state| {
            let job = state.jobs.get_mut(job_id).ok_or_else(|| {
                ProtocolError::NotFound(format!("worker job not found: {job_id}"))
            })?;
            if job.phase.terminal() {
                return Ok(false);
            }
            if job.cancel_requested_at.is_some() {
                return Ok(false);
            }
            job.cancel_requested_at = Some(now);
            if job.phase == JobPhase::Pending {
                job.phase = JobPhase::Cancelled;
            } else {
                job.phase = JobPhase::Cancelling;
            }
            Ok(true)
        })
    }
    pub fn confirm_cancelled(
        &self,
        worker_id: &str,
        job_id: &str,
        attempt: u32,
        fence: u64,
        now: u64,
    ) -> Result<(), ProtocolError> {
        self.store.transact(|state| {
            let job = current_job_mut_allow_cancelling(
                state.jobs.get_mut(job_id),
                worker_id,
                attempt,
                fence,
            )?;
            if job.phase != JobPhase::Cancelling {
                return Err(ProtocolError::Conflict(format!(
                    "job {job_id} is not cancelling"
                )));
            }
            job.phase = JobPhase::Cancelled;
            job.lease = None;
            if let Some(current) = job.attempts.last_mut() {
                current.phase = AttemptPhase::Cancelled;
                current.updated_at = now;
            }
            if let Some(worker) = state.workers.get_mut(worker_id) {
                worker.running = worker.running.saturating_sub(1);
            }
            Ok(())
        })
    }
    pub fn fail(
        &self,
        worker_id: &str,
        job_id: &str,
        attempt: u32,
        fence: u64,
        error: &str,
        now: u64,
    ) -> Result<(), ProtocolError> {
        self.store.transact(|state| {
            let job = current_job_mut(state.jobs.get_mut(job_id), worker_id, attempt, fence, now)?;
            if job.phase == JobPhase::Cancelling {
                return Err(ProtocolError::Cancelled(format!(
                    "job {job_id} is cancelling"
                )));
            }
            job.phase = JobPhase::Failed;
            job.lease = None;
            job.diagnostics.push(error.to_string());
            if let Some(current) = job.attempts.last_mut() {
                current.phase = AttemptPhase::Failed;
                current.updated_at = now;
            }
            if let Some(worker) = state.workers.get_mut(worker_id) {
                worker.running = worker.running.saturating_sub(1);
            }
            Ok(())
        })
    }
    pub fn expire_leases(&self, now: u64) -> Result<Vec<String>, ProtocolError> {
        self.store.transact(|state| {
            let expired = state
                .jobs
                .iter()
                .filter_map(|(id, job)| {
                    job.lease
                        .as_ref()
                        .filter(|lease| lease.expires_at <= now && !job.phase.terminal())
                        .map(|_| id.clone())
                })
                .collect::<Vec<_>>();
            for id in &expired {
                let job = state.jobs.get_mut(id).expect("collected job exists");
                let lease = job.lease.take().expect("collected lease exists");
                if let Some(attempt) = job.attempts.last_mut() {
                    attempt.phase = AttemptPhase::Failed;
                    attempt.updated_at = now;
                }
                if job.cancel_requested_at.is_some() {
                    job.phase = JobPhase::Cancelled;
                } else {
                    job.phase = JobPhase::Pending;
                }
                job.diagnostics.push(format!(
                    "lease {} expired at {}",
                    lease.fencing_token, lease.expires_at
                ));
                if let Some(worker) = state.workers.get_mut(&lease.worker_id) {
                    worker.running = worker.running.saturating_sub(1);
                }
            }
            Ok(expired)
        })
    }
    pub fn adopt(
        &self,
        worker_id: &str,
        job_id: &str,
        attempt: u32,
        fence: u64,
        now: u64,
    ) -> Result<WorkOffer, ProtocolError> {
        self.store.transact(|state| {
            let worker = state.workers.get(worker_id).ok_or_else(|| {
                ProtocolError::NotFound(format!("worker not registered: {worker_id}"))
            })?;
            let job = current_job_mut_allow_cancelling(
                state.jobs.get_mut(job_id),
                worker_id,
                attempt,
                fence,
            )?;
            let lease = job.lease.as_mut().expect("validated lease");
            if lease.expires_at <= now {
                return Err(ProtocolError::LeaseLost(format!(
                    "lease for {job_id} expired"
                )));
            }
            lease.heartbeat_at = now;
            lease.expires_at = now.saturating_add(self.lease_ms);
            Ok(WorkOffer {
                protocol: worker.negotiated,
                job: job.job.clone(),
                attempt,
                fencing_token: fence,
                lease_expires_at: lease.expires_at,
            })
        })
    }
}
