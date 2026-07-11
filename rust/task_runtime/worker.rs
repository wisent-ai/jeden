use super::cas::{build_snapshot, materialize_snapshot, LocalCas};
use super::coordinator::Coordinator;
use super::protocol::{
    AttemptPhase, CommitRequest, JobOutcome, ProtocolError, ProtocolVersion, WorkOffer,
    WorkerEvent, WorkerHello,
};
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;
use std::fs;
use std::path::Path;
use std::sync::{Arc, Mutex};

pub trait WorkerExecutor: Send + Sync {
    fn execute(
        &self,
        input: &Path,
        output: &Path,
        payload: &[u8],
        cancelled: &dyn Fn() -> bool,
    ) -> Result<Vec<u8>, String>;
}
impl<F> WorkerExecutor for F
where
    F: Fn(&Path, &Path, &[u8], &dyn Fn() -> bool) -> Result<Vec<u8>, String> + Send + Sync,
{
    fn execute(
        &self,
        input: &Path,
        output: &Path,
        payload: &[u8],
        cancelled: &dyn Fn() -> bool,
    ) -> Result<Vec<u8>, String> {
        self(input, output, payload, cancelled)
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case", tag = "status")]
pub enum WorkerRun {
    Completed {
        events: Vec<WorkerEvent>,
        commit: CommitRequest,
    },
    Cancelled {
        events: Vec<WorkerEvent>,
    },
    Failed {
        events: Vec<WorkerEvent>,
        error: String,
    },
}

#[derive(Clone)]
pub struct WorkerRuntime {
    pub hello: WorkerHello,
    pub cas: LocalCas,
    root: std::path::PathBuf,
    executor: Arc<dyn WorkerExecutor>,
    cancellations: Arc<Mutex<BTreeSet<(String, u32, u64)>>>,
}
impl std::fmt::Debug for WorkerRuntime {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("WorkerRuntime")
            .field("worker_id", &self.hello.worker_id)
            .field("root", &self.root)
            .finish()
    }
}
impl WorkerRuntime {
    pub fn open(
        root: impl AsRef<Path>,
        hello: WorkerHello,
        cas: LocalCas,
        executor: Arc<dyn WorkerExecutor>,
    ) -> Result<Self, ProtocolError> {
        let root = root.as_ref().to_path_buf();
        fs::create_dir_all(&root).map_err(|e| ProtocolError::Storage(e.to_string()))?;
        Ok(Self {
            hello,
            cas,
            root,
            executor,
            cancellations: Arc::new(Mutex::new(BTreeSet::new())),
        })
    }
    pub fn cancel(&self, job_id: &str, attempt: u32, fence: u64) -> Result<bool, ProtocolError> {
        let mut cancellations = self
            .cancellations
            .lock()
            .map_err(|_| ProtocolError::Transport("worker cancellation lock poisoned".into()))?;
        Ok(cancellations.insert((job_id.into(), attempt, fence)))
    }
    pub fn execute(&self, offer: WorkOffer) -> Result<WorkerRun, ProtocolError> {
        if offer.protocol.major != ProtocolVersion::V1.major
            || offer.protocol.minor > ProtocolVersion::V1.minor
        {
            return Err(ProtocolError::UnsupportedVersion {
                minimum: offer.protocol,
                maximum: offer.protocol,
            });
        }
        let key = (offer.job.id.clone(), offer.attempt, offer.fencing_token);
        let cancelled = || {
            self.cancellations
                .lock()
                .map(|set| set.contains(&key))
                .unwrap_or(true)
        };
        let mut events = Vec::new();
        let mut sequence = 0_u64;
        macro_rules! event {
            ($phase:expr, $detail:expr) => {{
                sequence = sequence.saturating_add(1);
                events.push(WorkerEvent {
                    job_id: offer.job.id.clone(),
                    attempt: offer.attempt,
                    fencing_token: offer.fencing_token,
                    sequence,
                    phase: $phase,
                    detail: $detail.into(),
                });
            }};
        }
        if cancelled() {
            event!(AttemptPhase::Cancelled, "cancelled before materialization");
            return Ok(WorkerRun::Cancelled { events });
        }
        event!(AttemptPhase::Materializing, "materializing input snapshot");
        let run_root = self.root.join(format!(
            "{}-{}-{}",
            safe_id(&offer.job.id),
            offer.attempt,
            offer.fencing_token
        ));
        if run_root.exists() {
            fs::remove_dir_all(&run_root).map_err(|e| ProtocolError::Storage(e.to_string()))?;
        }
        let input = run_root.join("input");
        let output = run_root.join("output");
        fs::create_dir_all(&run_root).map_err(|e| ProtocolError::Storage(e.to_string()))?;
        if let Err(error) = materialize_snapshot(&self.cas, offer.job.input_root, &input) {
            event!(AttemptPhase::Failed, "input materialization failed");
            return Ok(WorkerRun::Failed {
                events,
                error: error.to_string(),
            });
        }
        fs::create_dir(&output).map_err(|e| ProtocolError::Storage(e.to_string()))?;
        if cancelled() {
            event!(AttemptPhase::Cancelled, "cancelled after materialization");
            return Ok(WorkerRun::Cancelled { events });
        }
        event!(AttemptPhase::Running, "executor started");
        let result = match self
            .executor
            .execute(&input, &output, &offer.job.payload, &cancelled)
        {
            Ok(result) => result,
            Err(_error) if cancelled() => {
                event!(AttemptPhase::Cancelled, "cancelled while running");
                return Ok(WorkerRun::Cancelled { events });
            }
            Err(error) => {
                event!(AttemptPhase::Failed, "executor failed");
                return Ok(WorkerRun::Failed { events, error });
            }
        };
        if cancelled() {
            event!(AttemptPhase::Cancelled, "cancelled before upload");
            return Ok(WorkerRun::Cancelled { events });
        }
        event!(AttemptPhase::Uploading, "capturing output snapshot");
        let output_root = match build_snapshot(&self.cas, &output) {
            Ok(digest) => digest,
            Err(error) => {
                event!(AttemptPhase::Failed, "output capture failed");
                return Ok(WorkerRun::Failed {
                    events,
                    error: error.to_string(),
                });
            }
        };
        if cancelled() {
            event!(AttemptPhase::Cancelled, "cancelled before commit");
            return Ok(WorkerRun::Cancelled { events });
        }
        event!(AttemptPhase::CommitReady, "output committed to CAS");
        Ok(WorkerRun::Completed {
            events,
            commit: CommitRequest {
                job_id: offer.job.id,
                attempt: offer.attempt,
                fencing_token: offer.fencing_token,
                output_root,
                result,
            },
        })
    }
}

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
fn safe_id(value: &str) -> String {
    value
        .chars()
        .map(|character| {
            if character.is_ascii_alphanumeric() || matches!(character, '-' | '_') {
                character
            } else {
                '_'
            }
        })
        .collect()
}
