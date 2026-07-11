use super::protocol::{
    Attempt, Job, JobOutcome, JobPhase, Lease, ProtocolError, Worker, WorkerEvent,
};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

pub const COORDINATOR_STORE_SCHEMA_VERSION: u32 = 1;

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
            coordinator_epoch: 1,
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
        fs::create_dir_all(root).map_err(storage)?;
        let path = root.join("coordinator-v1.json");
        let mut state = if path.exists() {
            serde_json::from_slice::<CoordinatorState>(&fs::read(&path).map_err(storage)?).map_err(
                |e| {
                    ProtocolError::Storage(format!(
                        "read coordinator state {}: {e}",
                        path.display()
                    ))
                },
            )?
        } else {
            CoordinatorState::default()
        };
        if state.schema_version != COORDINATOR_STORE_SCHEMA_VERSION {
            return Err(ProtocolError::Storage(format!(
                "unsupported coordinator store schema {}",
                state.schema_version
            )));
        }
        state.coordinator_epoch = state.coordinator_epoch.saturating_add(1);
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
            .map_err(|_| ProtocolError::Storage("coordinator store lock poisoned".into()))?;
        f(&state)
    }
    pub(crate) fn transact<T>(
        &self,
        f: impl FnOnce(&mut CoordinatorState) -> Result<T, ProtocolError>,
    ) -> Result<T, ProtocolError> {
        let mut state = self
            .state
            .lock()
            .map_err(|_| ProtocolError::Storage("coordinator store lock poisoned".into()))?;
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
            .map_err(|_| ProtocolError::Storage("coordinator store lock poisoned".into()))?;
        write_atomic(&self.path, &state)
    }
}

fn write_atomic(path: &Path, value: &CoordinatorState) -> Result<(), ProtocolError> {
    let temporary = path.with_extension(format!("tmp-{}", std::process::id()));
    let bytes =
        serde_json::to_vec_pretty(value).map_err(|e| ProtocolError::Storage(e.to_string()))?;
    {
        use std::io::Write;
        let mut file = fs::OpenOptions::new()
            .create(true)
            .truncate(true)
            .write(true)
            .open(&temporary)
            .map_err(storage)?;
        file.write_all(&bytes).map_err(storage)?;
        file.write_all(b"\n").map_err(storage)?;
        file.sync_all().map_err(storage)?;
    }
    fs::rename(&temporary, path).map_err(storage)?;
    if let Some(parent) = path.parent() {
        fs::File::open(parent)
            .and_then(|file| file.sync_all())
            .map_err(storage)?;
    }
    Ok(())
}
fn storage(error: std::io::Error) -> ProtocolError {
    ProtocolError::Storage(error.to_string())
}
