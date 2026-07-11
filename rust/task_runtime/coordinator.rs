use super::cas::LocalCas;
use super::placement::{select_worker, PlacementDecision};
use super::protocol::{
    negotiate_version, Attempt, AttemptPhase, CommitRequest, Job, JobOutcome, JobPhase, Lease,
    NegotiatedHello, ProtocolError, WorkOffer, Worker, WorkerEvent, WorkerHello,
};
use super::store::{CoordinatorStore, JobState};
use std::path::Path;

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
