use crate::task_runtime::protocol::{
    Attempt, Job, JobOutcome, JobPhase, Lease, ProtocolError, Worker, WorkerEvent,
};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

/// The first schema of the coordinator store, the one this build reads and writes.
pub const COORDINATOR_STORE_SCHEMA_VERSION: u32 = crate::tally::FIRST as u32;

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct JobState {
    pub job: Job,
    pub phase: JobPhase,
    #[serde(default)]
    pub attempts: Vec<Attempt>,
    pub lease: Option<Lease>,
    pub outcome: Option<JobOutcome>,
    #[serde(default, with = "event_map")]
    pub events: BTreeMap<(u32, u64), WorkerEvent>,
    #[serde(default)]
    pub next_fencing_token: u64,
    #[serde(default)]
    pub cancel_requested_at: Option<u64>,
    #[serde(default)]
    pub diagnostics: Vec<String>,
}

mod event_map {
    use super::WorkerEvent;
    use serde::de::Error as _;
    use serde::{Deserialize, Deserializer, Serialize, Serializer};
    use std::collections::BTreeMap;

    pub fn serialize<S>(
        events: &BTreeMap<(u32, u64), WorkerEvent>,
        serializer: S,
    ) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        events.values().collect::<Vec<_>>().serialize(serializer)
    }
    pub fn deserialize<'de, D>(
        deserializer: D,
    ) -> Result<BTreeMap<(u32, u64), WorkerEvent>, D::Error>
    where
        D: Deserializer<'de>,
    {
        let events = Vec::<WorkerEvent>::deserialize(deserializer)?;
        let mut ordered = BTreeMap::new();
        for event in events {
            let key = (event.attempt, event.sequence);
            if ordered.insert(key, event).is_some() {
                return Err(D::Error::custom(format!(
                    "duplicate worker event {}:{}",
                    key.0, key.1
                )));
            }
        }
        Ok(ordered)
    }
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct CoordinatorState {
    pub schema_version: u32,
    pub coordinator_epoch: u64,
    #[serde(default)]
    pub workers: BTreeMap<String, Worker>,
    #[serde(default)]
    pub jobs: BTreeMap<String, JobState>,
}
impl Default for CoordinatorState {
    fn default() -> Self {
        Self {
            schema_version: COORDINATOR_STORE_SCHEMA_VERSION,
            coordinator_epoch: crate::tally::FIRST,
            workers: BTreeMap::new(),
            jobs: BTreeMap::new(),
        }
    }
}

#[derive(Clone, Debug)]
pub struct CoordinatorStore {
    path: PathBuf,
    state: Arc<Mutex<CoordinatorState>>,
}
impl CoordinatorStore {
    pub fn open(root: impl AsRef<Path>) -> Result<Self, ProtocolError> {
        let root = root.as_ref();
        fs::create_dir_all(root).map_err(|error| {
            ProtocolError::Storage(format!("create coordinator directory {}: {error}", root.display()))
        })?;
        let path = root.join("coordinator-v1.json");
        // Only a state that does not exist starts fresh: one that could not
        // be stat'ed did too, and the epoch and leases restarted from zero.
        let mut state = match fs::read(&path) {
            Ok(bytes) => serde_json::from_slice::<CoordinatorState>(&bytes).map_err(|e| {
                ProtocolError::Storage(format!("read coordinator state {}: {e}", path.display()))
            })?,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                CoordinatorState::default()
            }
            Err(error) => {
                return Err(ProtocolError::Storage(format!(
                    "read coordinator state {}: {error}",
                    path.display()
                )))
            }
        };
        if state.schema_version != COORDINATOR_STORE_SCHEMA_VERSION {
            return Err(ProtocolError::Storage(format!(
                "unsupported coordinator store schema {}",
                state.schema_version
            )));
        }
        // The epoch fences a restarted coordinator from the leases of the one
        // before it, so it must grow on every open; one that cannot grow is
        // refused instead of reused.
        state.coordinator_epoch = state
            .coordinator_epoch
            .checked_add(u64::from(crate::tally::STEP))
            .ok_or_else(|| {
                ProtocolError::Storage(format!(
                    "coordinator state {} is at the last epoch {}; it cannot fence another start",
                    path.display(),
                    state.coordinator_epoch
                ))
            })?;
        let store = Self {
            path,
            state: Arc::new(Mutex::new(state)),
        };
        store.persist()?;
        Ok(store)
    }
    pub fn path(&self) -> &Path {
        &self.path
    }
    pub fn job(&self, id: &str) -> Result<JobState, ProtocolError> {
        self.read(|state| {
            state
                .jobs
                .get(id)
                .cloned()
                .ok_or_else(|| ProtocolError::NotFound(format!("worker job not found: {id}")))
        })
    }
    pub fn jobs(&self) -> Result<Vec<JobState>, ProtocolError> {
        self.read(|state| Ok(state.jobs.values().cloned().collect()))
    }
    pub(crate) fn read<T>(
        &self,
        f: impl FnOnce(&CoordinatorState) -> Result<T, ProtocolError>,
    ) -> Result<T, ProtocolError> {
        let state = self
            .state
            .lock()
            .map_err(|poisoned| ProtocolError::Storage(format!("coordinator store lock: {poisoned}")))?;
        f(&state)
    }
    pub(crate) fn transact<T>(
        &self,
        f: impl FnOnce(&mut CoordinatorState) -> Result<T, ProtocolError>,
    ) -> Result<T, ProtocolError> {
        let mut state = self
            .state
            .lock()
            .map_err(|poisoned| ProtocolError::Storage(format!("coordinator store lock: {poisoned}")))?;
        let mut next = state.clone();
        let output = f(&mut next)?;
        write_atomic(&self.path, &next)?;
        *state = next;
        Ok(output)
    }
    fn persist(&self) -> Result<(), ProtocolError> {
        let state = self
            .state
            .lock()
            .map_err(|poisoned| ProtocolError::Storage(format!("coordinator store lock: {poisoned}")))?;
        write_atomic(&self.path, &state)
    }
}

/// The coordinator state over `path` through the shared durable replace. The
/// staging file was named by the process id, so two coordinators of one
/// process wrote into one staging file.
fn write_atomic(path: &Path, value: &CoordinatorState) -> Result<(), ProtocolError> {
    let mut bytes = serde_json::to_vec_pretty(value).map_err(|error| {
        ProtocolError::Storage(format!("encode coordinator state {}: {error}", path.display()))
    })?;
    bytes.push(b'\n');
    crate::tool_runtime::runtime_ops::durable::replace(path, &bytes).map_err(ProtocolError::Storage)
}
