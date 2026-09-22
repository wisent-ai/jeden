//! How a worker is reached, and the in-process path used when it runs beside
//! the coordinator.
//!
//! Split out of `task_runtime/fleet/worker.rs`, which had grown past the
//! module line cap.

use super::{WorkerRun, WorkerRuntime};
use crate::task_runtime::fleet::coordinator::Coordinator;
use crate::task_runtime::fleet::protocol::{JobOutcome, ProtocolError, WorkOffer, WorkerHello};
use serde::{Deserialize, Serialize};

pub trait WorkerTransport {
    fn hello(&self) -> Result<WorkerHello, ProtocolError>;
    fn execute(&self, offer: WorkOffer) -> Result<WorkerRun, ProtocolError>;
    fn cancel(&self, job_id: &str, attempt: u32, fence: u64) -> Result<bool, ProtocolError>;
}

#[derive(Clone, Debug)]
pub struct LoopbackTransport {
    worker: WorkerRuntime,
    serialize_boundary: bool,
}
impl LoopbackTransport {
    pub fn local(worker: WorkerRuntime) -> Self {
        Self {
            worker,
            serialize_boundary: false,
        }
    }
    pub fn remote(worker: WorkerRuntime) -> Self {
        Self {
            worker,
            serialize_boundary: true,
        }
    }
    pub fn run(
        &self,
        coordinator: &Coordinator,
        job_id: &str,
        now: u64,
    ) -> Result<JobOutcome, ProtocolError> {
        coordinator.register_worker(self.hello()?, now)?;
        let offer = coordinator.assign(job_id, now)?;
        coordinator.acknowledge(
            &self.worker.hello.worker_id,
            job_id,
            offer.attempt,
            offer.fencing_token,
            now,
        )?;
        let run = self.execute(offer)?;
        let events = match &run {
            WorkerRun::Completed { events, .. }
            | WorkerRun::Cancelled { events }
            | WorkerRun::Failed { events, .. } => events,
        };
        for event in events {
            coordinator.record_event(&self.worker.hello.worker_id, event.clone(), now)?;
        }
        match run {
            WorkerRun::Completed { commit, .. } => {
                coordinator.commit(&self.worker.hello.worker_id, commit, now)
            }
            WorkerRun::Cancelled { events } => {
                let last = events.last().ok_or_else(|| {
                    ProtocolError::Transport("cancelled run emitted no event".into())
                })?;
                coordinator.cancel(job_id, now)?;
                coordinator.confirm_cancelled(
                    &self.worker.hello.worker_id,
                    job_id,
                    last.attempt,
                    last.fencing_token,
                    now,
                )?;
                Err(ProtocolError::Cancelled(format!("job {job_id} cancelled")))
            }
            WorkerRun::Failed { events, error } => {
                let last = events.last().ok_or_else(|| {
                    ProtocolError::Transport("failed run emitted no event".into())
                })?;
                coordinator.fail(
                    &self.worker.hello.worker_id,
                    job_id,
                    last.attempt,
                    last.fencing_token,
                    &error,
                    now,
                )?;
                Err(ProtocolError::Transport(error))
            }
        }
    }
}
impl WorkerTransport for LoopbackTransport {
    fn hello(&self) -> Result<WorkerHello, ProtocolError> {
        round_trip(&self.worker.hello, self.serialize_boundary)
    }
    fn execute(&self, offer: WorkOffer) -> Result<WorkerRun, ProtocolError> {
        let offer = round_trip(&offer, self.serialize_boundary)?;
        round_trip(&self.worker.execute(offer)?, self.serialize_boundary)
    }
    fn cancel(&self, job_id: &str, attempt: u32, fence: u64) -> Result<bool, ProtocolError> {
        self.worker.cancel(job_id, attempt, fence)
    }
}

fn round_trip<T>(value: &T, enabled: bool) -> Result<T, ProtocolError>
where
    T: Serialize + for<'de> Deserialize<'de> + Clone,
{
    if !enabled {
        return Ok(value.clone());
    }
    let bytes = serde_json::to_vec(value).map_err(|e| ProtocolError::Transport(e.to_string()))?;
    serde_json::from_slice(&bytes).map_err(|e| ProtocolError::Transport(e.to_string()))
}
