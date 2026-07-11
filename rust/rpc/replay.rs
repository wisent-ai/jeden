use super::tenant::TenantId;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::fs::{self, OpenOptions};
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct SessionEventV1 {
    pub session_id: String,
    pub stream_id: String,
    pub sequence: u64,
    pub event_id: String,
    pub request_id: String,
    pub kind: String,
    pub payload: Value,
    pub terminal: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EventCursor(pub u64);

impl EventCursor {
    pub fn parse(token: &str) -> Result<Self, ReplayError> {
        let value = token
            .strip_prefix("cursor-")
            .ok_or(ReplayError::InvalidCursor)?;
        if value.is_empty() || !value.bytes().all(|byte| byte.is_ascii_digit()) {
            return Err(ReplayError::InvalidCursor);
        }
        value
            .parse::<u64>()
            .map(Self)
            .map_err(|_| ReplayError::InvalidCursor)
    }

    pub fn token(self) -> String {
        format!("cursor-{:020}", self.0)
    }
}

impl SessionEventV1 {
    pub fn cursor(&self) -> String {
        EventCursor(self.sequence).token()
    }

    pub fn wire_value(&self) -> Value {
        serde_json::json!({
            "type": "event",
            "sessionId": self.session_id,
            "streamId": self.stream_id,
            "sequence": self.sequence,
            "cursor": self.cursor(),
            "eventId": self.event_id,
            "requestId": self.request_id,
            "kind": self.kind,
            "payload": self.payload,
            "terminal": self.terminal,
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ReplayError {
    InvalidStream,
    InvalidCursor,
    CursorAhead { latest: u64 },
    CursorTooOld { earliest: u64 },
    SequenceConflict { expected: u64 },
    TerminalAlreadyRecorded,
    CorruptLog(String),
    Storage(String),
}

#[derive(Debug, Default)]
struct StreamState {
    events: Vec<SessionEventV1>,
    terminal_by_request: HashMap<String, u64>,
}

#[derive(Debug, Clone)]
pub struct ReplayStore {
    root: PathBuf,
    retention_events: usize,
    streams: Arc<Mutex<HashMap<PathBuf, StreamState>>>,
}

impl ReplayStore {
    pub fn new(root: impl Into<PathBuf>, retention_events: usize) -> Self {
        Self {
            root: root.into(),
            retention_events: retention_events.max(1),
            streams: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    pub fn append(
        &self,
        tenant: &TenantId,
        mut event: SessionEventV1,
    ) -> Result<SessionEventV1, ReplayError> {
        validate_name(&event.session_id)?;
        validate_name(&event.stream_id)?;
        if event.request_id.is_empty() || event.kind.is_empty() {
            return Err(ReplayError::InvalidStream);
        }
        let path = self.stream_path(tenant, &event.session_id, &event.stream_id);
        let mut streams = self
            .streams
            .lock()
            .map_err(|_| ReplayError::Storage("replay lock poisoned".into()))?;
        if !streams.contains_key(&path) {
            streams.insert(path.clone(), load_stream(&path)?);
        }
        let state = streams.get_mut(&path).expect("stream inserted");
        if state.terminal_by_request.contains_key(&event.request_id) {
            return Err(ReplayError::TerminalAlreadyRecorded);
        }
        let expected = state
            .events
            .last()
            .map_or(1, |stored| stored.sequence.saturating_add(1));
        if event.sequence != 0 && event.sequence != expected {
            return Err(ReplayError::SequenceConflict { expected });
        }
        event.sequence = expected;
        if event.event_id.is_empty() {
            event.event_id = format!("{}:{}:{}", event.session_id, event.stream_id, expected);
        }
        append_synced(&path, &event)?;
        if event.terminal {
            state
                .terminal_by_request
                .insert(event.request_id.clone(), expected);
        }
        state.events.push(event.clone());
        if state.events.len() > self.retention_events {
            let remove = state.events.len() - self.retention_events;
            state.events.drain(..remove);
        }
        Ok(event)
    }

    pub fn replay(
        &self,
        tenant: &TenantId,
        session_id: &str,
        stream_id: &str,
        after: EventCursor,
        limit: usize,
    ) -> Result<Vec<SessionEventV1>, ReplayError> {
        validate_name(session_id)?;
        validate_name(stream_id)?;
        let path = self.stream_path(tenant, session_id, stream_id);
        let mut streams = self
            .streams
            .lock()
            .map_err(|_| ReplayError::Storage("replay lock poisoned".into()))?;
        if !streams.contains_key(&path) {
            streams.insert(path.clone(), load_stream(&path)?);
        }
        let state = streams.get(&path).expect("stream inserted");
        let latest = state.events.last().map_or(0, |event| event.sequence);
        if after.0 > latest {
            return Err(ReplayError::CursorAhead { latest });
        }
        let earliest = state.events.first().map_or(1, |event| event.sequence);
        if after.0.saturating_add(1) < earliest {
            return Err(ReplayError::CursorTooOld { earliest });
        }
        Ok(state
            .events
            .iter()
            .filter(|event| event.sequence > after.0)
            .take(limit.max(1))
            .cloned()
            .collect())
    }

    pub fn latest_cursor(
        &self,
        tenant: &TenantId,
        session_id: &str,
        stream_id: &str,
    ) -> Result<EventCursor, ReplayError> {
        let events = self.replay(tenant, session_id, stream_id, EventCursor(0), usize::MAX);
        match events {
            Ok(events) => Ok(EventCursor(events.last().map_or(0, |event| event.sequence))),
            Err(ReplayError::CursorTooOld { earliest }) => {
                let events = self.replay(
                    tenant,
                    session_id,
                    stream_id,
                    EventCursor(earliest.saturating_sub(1)),
                    usize::MAX,
                )?;
                Ok(EventCursor(events.last().map_or(0, |event| event.sequence)))
            }
            Err(error) => Err(error),
        }
    }

    fn stream_path(&self, tenant: &TenantId, session_id: &str, stream_id: &str) -> PathBuf {
        self.root
            .join(digest(tenant.as_str()))
            .join(digest(session_id))
            .join(format!("{}.jsonl", digest(stream_id)))
    }
}

fn load_stream(path: &Path) -> Result<StreamState, ReplayError> {
    if !path.exists() {
        return Ok(StreamState::default());
    }
    let file = OpenOptions::new().read(true).open(path).map_err(storage)?;
    let mut state = StreamState::default();
    for (index, line) in BufReader::new(file).lines().enumerate() {
        let line = line.map_err(storage)?;
        let event: SessionEventV1 = serde_json::from_str(&line)
            .map_err(|error| ReplayError::CorruptLog(format!("line {}: {}", index + 1, error)))?;
        let expected = state
            .events
            .last()
            .map_or(1, |prior: &SessionEventV1| prior.sequence + 1);
        if event.sequence != expected {
            return Err(ReplayError::CorruptLog(format!(
                "line {} sequence",
                index + 1
            )));
        }
        if event.terminal
            && state
                .terminal_by_request
                .insert(event.request_id.clone(), event.sequence)
                .is_some()
        {
            return Err(ReplayError::CorruptLog(format!(
                "line {} duplicate terminal",
                index + 1
            )));
        }
        state.events.push(event);
    }
    Ok(state)
}

fn append_synced(path: &Path, event: &SessionEventV1) -> Result<(), ReplayError> {
    let parent = path
        .parent()
        .ok_or_else(|| ReplayError::Storage("stream has no parent".into()))?;
    fs::create_dir_all(parent).map_err(storage)?;
    let mut file = OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
        .map_err(storage)?;
    serde_json::to_writer(&mut file, event)
        .map_err(|error| ReplayError::Storage(error.to_string()))?;
    file.write_all(b"\n").map_err(storage)?;
    file.sync_data().map_err(storage)
}

fn validate_name(value: &str) -> Result<(), ReplayError> {
    if value.is_empty() || value.len() > 512 {
        Err(ReplayError::InvalidStream)
    } else {
        Ok(())
    }
}
fn digest(value: &str) -> String {
    hex::encode(Sha256::digest(value.as_bytes()))
}
fn storage(error: std::io::Error) -> ReplayError {
    ReplayError::Storage(error.to_string())
}

#[cfg(test)]
mod tests {
    use super::super::tenant::TenantDirectory;
    use super::super::tls::{PeerCertificate, VerifiedPeer};
    use super::*;
    use serde_json::json;
    use std::sync::atomic::{AtomicU64, Ordering};

    static NEXT_DIRECTORY: AtomicU64 = AtomicU64::new(0);

    struct TempDirectory(PathBuf);

    impl TempDirectory {
        fn new() -> Self {
            let suffix = NEXT_DIRECTORY.fetch_add(1, Ordering::Relaxed);
            let path = std::env::temp_dir()
                .join(format!("jeden-replay-test-{}-{suffix}", std::process::id()));
            let _ = fs::remove_dir_all(&path);
            fs::create_dir_all(&path).unwrap();
            Self(path)
        }
    }

    impl Drop for TempDirectory {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    fn tenant() -> TenantId {
        let directory = TenantDirectory::new();
        directory
            .map_san("spiffe://tenant-a/client", "principal-a", "tenant-a")
            .unwrap();
        directory
            .resolve(&VerifiedPeer {
                certificate: PeerCertificate {
                    serial: "serial-1".into(),
                    issuer_fingerprint: "ca:test".into(),
                    dns_sans: Vec::new(),
                    uri_sans: vec!["spiffe://tenant-a/client".into()],
                    not_before_unix: 0,
                    not_after_unix: u64::MAX,
                },
                trust_generation: 1,
            })
            .unwrap()
            .tenant
    }

    fn event(request_id: &str, terminal: bool) -> SessionEventV1 {
        SessionEventV1 {
            session_id: "session-1".into(),
            stream_id: "stream-1".into(),
            sequence: 0,
            event_id: String::new(),
            request_id: request_id.into(),
            kind: if terminal { "completed" } else { "progress" }.into(),
            payload: json!({"request": request_id}),
            terminal,
        }
    }

    #[test]
    fn replay_enforces_retained_cursor_boundaries_and_limit() {
        let directory = TempDirectory::new();
        let tenant = tenant();
        let store = ReplayStore::new(&directory.0, 3);
        for request in ["one", "two", "three", "four"] {
            store.append(&tenant, event(request, false)).unwrap();
        }

        assert_eq!(
            store.replay(&tenant, "session-1", "stream-1", EventCursor(0), 10),
            Err(ReplayError::CursorTooOld { earliest: 2 })
        );
        let page = store
            .replay(&tenant, "session-1", "stream-1", EventCursor(1), 2)
            .unwrap();
        assert_eq!(
            page.iter().map(|item| item.sequence).collect::<Vec<_>>(),
            vec![2, 3]
        );
        assert_eq!(
            store.replay(&tenant, "session-1", "stream-1", EventCursor(4), 10),
            Ok(Vec::new())
        );
        assert_eq!(
            store.replay(&tenant, "session-1", "stream-1", EventCursor(5), 10),
            Err(ReplayError::CursorAhead { latest: 4 })
        );
    }

    #[test]
    fn terminal_uniqueness_and_events_survive_store_restart() {
        let directory = TempDirectory::new();
        let tenant = tenant();
        let first = ReplayStore::new(&directory.0, 10);
        first.append(&tenant, event("request-1", false)).unwrap();
        let terminal = first.append(&tenant, event("request-1", true)).unwrap();
        assert_eq!(terminal.sequence, 2);
        assert_eq!(
            first.append(&tenant, event("request-1", true)),
            Err(ReplayError::TerminalAlreadyRecorded)
        );
        drop(first);

        let restarted = ReplayStore::new(&directory.0, 10);
        let persisted = restarted
            .replay(&tenant, "session-1", "stream-1", EventCursor(0), 10)
            .unwrap();
        assert_eq!(
            persisted
                .iter()
                .map(|item| (item.sequence, item.terminal))
                .collect::<Vec<_>>(),
            vec![(1, false), (2, true)]
        );
        assert_eq!(
            restarted.append(&tenant, event("request-1", true)),
            Err(ReplayError::TerminalAlreadyRecorded)
        );
    }
}
