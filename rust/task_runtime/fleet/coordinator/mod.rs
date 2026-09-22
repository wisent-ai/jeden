mod lifecycle;
mod recovery;

pub mod placement;
pub mod store;

use super::protocol::{
    negotiate_version, Attempt, AttemptPhase, Job, JobPhase, Lease, NegotiatedHello, ProtocolError,
    WorkOffer, Worker, WorkerHello,
};
use crate::task_runtime::cas::LocalCas;
use placement::{select_worker, PlacementDecision};
use std::path::Path;
use store::{CoordinatorStore, JobState};

#[derive(Clone, Debug)]
pub struct Coordinator {
    pub store: CoordinatorStore,
    pub cas: LocalCas,
    lease_ms: u64,
}
impl Coordinator {
    pub fn open(root: impl AsRef<Path>, lease_ms: u64) -> Result<Self, ProtocolError> {
        let root = root.as_ref();
        let store = CoordinatorStore::open(root.join("state"))?;
        let cas =
            LocalCas::open(root.join("cas")).map_err(|e| ProtocolError::Storage(e.to_string()))?;
        Ok(Self {
            store,
            cas,
            lease_ms: lease_ms.clamp(100, 300_000),
        })
    }
    pub fn register_worker(
        &self,
        hello: WorkerHello,
        now: u64,
    ) -> Result<NegotiatedHello, ProtocolError> {
        if hello.worker_id.trim().is_empty() {
            return Err(ProtocolError::Invalid("worker id is required".into()));
        }
        let negotiated = negotiate_version(&hello.versions)?;
        self.store.transact(|state| {
            if let Some(previous) = state.workers.get(&hello.worker_id) {
                if hello.incarnation < previous.hello.incarnation {
                    return Err(ProtocolError::Conflict(format!(
                        "stale worker incarnation {}",
                        hello.incarnation
                    )));
                }
            }
            let running = state
                .jobs
                .values()
                .filter(|job| {
                    job.lease
                        .as_ref()
                        .map(|lease| lease.worker_id == hello.worker_id)
                        .unwrap_or(false)
                        && !job.phase.terminal()
                })
                .count() as u32;
            state.workers.insert(
                hello.worker_id.clone(),
                Worker {
                    hello: hello.clone(),
                    negotiated,
                    last_heartbeat: now,
                    running,
                },
            );
            Ok(NegotiatedHello {
                worker_id: hello.worker_id.clone(),
                version: negotiated,
                coordinator_epoch: state.coordinator_epoch,
            })
        })
    }
    pub fn submit(&self, job: Job) -> Result<JobState, ProtocolError> {
        if job.id.trim().is_empty() {
            return Err(ProtocolError::Invalid("job id is required".into()));
        }
        self.store.transact(|state| {
            if let Some(existing) = state.jobs.get(&job.id) {
                if existing.job == job {
                    return Ok(existing.clone());
                }
                return Err(ProtocolError::Conflict(format!(
                    "job id {} already exists with different content",
                    job.id
                )));
            }
            let record = JobState {
                job: job.clone(),
                phase: JobPhase::Pending,
                attempts: Vec::new(),
                lease: None,
                outcome: None,
                events: Default::default(),
                next_fencing_token: 1,
                cancel_requested_at: None,
                diagnostics: Vec::new(),
            };
            state.jobs.insert(job.id.clone(), record.clone());
            Ok(record)
        })
    }
    pub fn placement(&self, job_id: &str) -> Result<PlacementDecision, ProtocolError> {
        self.store.read(|state| {
            let job = state.jobs.get(job_id).ok_or_else(|| {
                ProtocolError::NotFound(format!("worker job not found: {job_id}"))
            })?;
            select_worker(&job.job, state.workers.values())
        })
    }
    pub fn assign(&self, job_id: &str, now: u64) -> Result<WorkOffer, ProtocolError> {
        let decision = self.placement(job_id)?;
        self.store.transact(|state| {
            let worker = state
                .workers
                .get_mut(&decision.worker_id)
                .ok_or_else(|| ProtocolError::NoPlacement("selected worker disappeared".into()))?;
            let job = state.jobs.get_mut(job_id).ok_or_else(|| {
                ProtocolError::NotFound(format!("worker job not found: {job_id}"))
            })?;
            if job.phase != JobPhase::Pending || job.lease.is_some() {
                return Err(ProtocolError::Conflict(format!(
                    "job {job_id} is not pending"
                )));
            }
            let number = job
                .attempts
                .last()
                .map(|attempt| attempt.number.saturating_add(1))
                .unwrap_or(1);
            let fence = job.next_fencing_token.max(1);
            job.next_fencing_token = fence.saturating_add(1);
            let expires_at = now.saturating_add(self.lease_ms);
            job.attempts.push(Attempt {
                job_id: job_id.into(),
                number,
                worker_id: decision.worker_id.clone(),
                fencing_token: fence,
                phase: AttemptPhase::Offered,
                started_at: now,
                updated_at: now,
            });
            job.lease = Some(Lease {
                job_id: job_id.into(),
                attempt: number,
                worker_id: decision.worker_id.clone(),
                fencing_token: fence,
                expires_at,
                heartbeat_at: now,
            });
            job.phase = JobPhase::Assigned;
            worker.running = worker.running.saturating_add(1);
            Ok(WorkOffer {
                protocol: worker.negotiated,
                job: job.job.clone(),
                attempt: number,
                fencing_token: fence,
                lease_expires_at: expires_at,
            })
        })
    }
    pub fn job(&self, id: &str) -> Result<JobState, ProtocolError> {
        self.store.job(id)
    }
}

fn current_job_mut<'a>(
    job: Option<&'a mut JobState>,
    worker_id: &str,
    attempt: u32,
    fence: u64,
    now: u64,
) -> Result<&'a mut JobState, ProtocolError> {
    let job = current_job_mut_allow_cancelling(job, worker_id, attempt, fence)?;
    let lease = job.lease.as_ref().expect("validated lease");
    if lease.expires_at <= now {
        return Err(ProtocolError::LeaseLost(format!(
            "lease for {} expired",
            lease.job_id
        )));
    }
    Ok(job)
}
fn current_job_mut_allow_cancelling<'a>(
    job: Option<&'a mut JobState>,
    worker_id: &str,
    attempt: u32,
    fence: u64,
) -> Result<&'a mut JobState, ProtocolError> {
    let job = job.ok_or_else(|| ProtocolError::NotFound("worker job not found".into()))?;
    let lease = job.lease.as_ref().ok_or_else(|| {
        ProtocolError::LeaseLost(format!("job {} has no active lease", job.job.id))
    })?;
    if lease.fencing_token != fence {
        return Err(ProtocolError::StaleFence {
            expected: lease.fencing_token,
            actual: fence,
        });
    }
    if lease.worker_id != worker_id || lease.attempt != attempt {
        return Err(ProtocolError::LeaseLost(format!(
            "lease for {} belongs to another attempt",
            job.job.id
        )));
    }
    Ok(job)
}
