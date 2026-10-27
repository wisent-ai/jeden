//! The replay journal: every event of a request's stream, kept on disk so a
//! client that lost its connection reads from where it stopped.

mod event;

pub use event::{EventCursor, EventDraft, ReplayError, SessionEventV1};

use super::tenant::TenantId;
use crate::tally::{FIRST, STEP};
use sha2::{Digest, Sha256};
use std::collections::hash_map::Entry;
use std::collections::HashMap;
use std::fs::{self, OpenOptions};
use std::io::{BufRead, BufReader, Write};
use std::num::NonZeroUsize;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

#[derive(Debug, Default)]
struct StreamState {
    events: Vec<SessionEventV1>,
    terminal_by_request: HashMap<String, u64>,
}

impl StreamState {
    /// The sequence the next event of this stream takes.
    fn next_sequence(&self) -> Result<u64, ReplayError> {
        match self.events.last() {
            None => Ok(FIRST),
            Some(stored) => stored.sequence.checked_add(u64::from(STEP)).ok_or_else(|| {
                ReplayError::Storage(format!(
                    "the stream of {} has used every sequence up to {}",
                    stored.request_id, stored.sequence
                ))
            }),
        }
    }
}

#[derive(Debug, Clone)]
pub struct ReplayStore {
    root: PathBuf,
    retention_events: NonZeroUsize,
    streams: Arc<Mutex<HashMap<PathBuf, StreamState>>>,
}

impl ReplayStore {
    pub fn new(root: impl Into<PathBuf>, retention_events: NonZeroUsize) -> Self {
        Self {
            root: root.into(),
            retention_events,
            streams: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    pub fn append(
        &self,
        tenant: &TenantId,
        draft: EventDraft,
    ) -> Result<SessionEventV1, ReplayError> {
        validate_name(&draft.session_id)?;
        validate_name(&draft.stream_id)?;
        if draft.request_id.is_empty() || draft.kind.is_empty() {
            return Err(ReplayError::InvalidStream);
        }
        let path = self.stream_path(tenant, &draft.session_id, &draft.stream_id);
        let mut streams = self
            .streams
            .lock()
            .map_err(|_| ReplayError::Storage("replay lock poisoned".into()))?;
        let state = loaded(&mut streams, path.clone())?;
        if state.terminal_by_request.contains_key(&draft.request_id) {
            return Err(ReplayError::TerminalAlreadyRecorded);
        }
        let sequence = state.next_sequence()?;
        let event = SessionEventV1 {
            event_id: format!("{}:{}:{}", draft.session_id, draft.stream_id, sequence),
            session_id: draft.session_id,
            stream_id: draft.stream_id,
            sequence,
            request_id: draft.request_id,
            kind: draft.kind,
            payload: draft.payload,
            terminal: draft.terminal,
        };
        append_synced(&path, &event)?;
        if event.terminal {
            state
                .terminal_by_request
                .insert(event.request_id.clone(), sequence);
        }
        state.events.push(event.clone());
        let excess = state
            .events
            .len()
            .saturating_sub(self.retention_events.get());
        state.events.drain(..excess);
        Ok(event)
    }

    pub fn replay(
        &self,
        tenant: &TenantId,
        session_id: &str,
        stream_id: &str,
        after: EventCursor,
        limit: Option<usize>,
    ) -> Result<Vec<SessionEventV1>, ReplayError> {
        validate_name(session_id)?;
        validate_name(stream_id)?;
        let path = self.stream_path(tenant, session_id, stream_id);
        let mut streams = self
            .streams
            .lock()
            .map_err(|_| ReplayError::Storage("replay lock poisoned".into()))?;
        let state = loaded(&mut streams, path)?;
        match (state.events.first(), state.events.last()) {
            (Some(first), Some(last)) => {
                if after.0 > last.sequence {
                    return Err(ReplayError::CursorAhead {
                        latest: last.sequence,
                    });
                }
                if after.0.saturating_add(u64::from(STEP)) < first.sequence {
                    return Err(ReplayError::CursorTooOld {
                        earliest: first.sequence,
                    });
                }
            }
            _ if after != EventCursor::default() => {
                return Err(ReplayError::CursorAhead {
                    latest: EventCursor::default().0,
                });
            }
            _ => {}
        }
        let after_cursor = state
            .events
            .iter()
            .filter(|event| event.sequence > after.0)
            .cloned();
        // Every event after the cursor, or the first `limit` the caller asked for.
        Ok(match limit {
            Some(limit) => after_cursor.take(limit).collect(),
            None => after_cursor.collect(),
        })
    }

    fn stream_path(&self, tenant: &TenantId, session_id: &str, stream_id: &str) -> PathBuf {
        self.root
            .join(digest(tenant.as_str()))
            .join(digest(session_id))
            .join(format!("{}.jsonl", digest(stream_id)))
    }
}

/// The stream at `path`, read from disk the first time it is asked for.
fn loaded(
    streams: &mut HashMap<PathBuf, StreamState>,
    path: PathBuf,
) -> Result<&mut StreamState, ReplayError> {
    match streams.entry(path) {
        Entry::Occupied(entry) => Ok(entry.into_mut()),
        Entry::Vacant(entry) => {
            let state = load_stream(entry.key())?;
            Ok(entry.insert(state))
        }
    }
}

fn load_stream(path: &Path) -> Result<StreamState, ReplayError> {
    // Only a stream that does not exist is empty; one that cannot be opened
    // for any other cause is the storage error, not a stream with no events.
    let file = match OpenOptions::new().read(true).open(path) {
        Ok(file) => file,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Ok(StreamState::default())
        }
        Err(error) => return Err(storage(error)),
    };
    let mut state = StreamState::default();
    for (line_number, line) in (FIRST..).zip(BufReader::new(file).lines()) {
        let line = line.map_err(storage)?;
        let event: SessionEventV1 = serde_json::from_str(&line)
            .map_err(|error| ReplayError::CorruptLog(format!("line {line_number}: {error}")))?;
        let expected = state.next_sequence()?;
        if event.sequence != expected {
            return Err(ReplayError::CorruptLog(format!(
                "line {line_number} holds sequence {}, where {expected} follows",
                event.sequence
            )));
        }
        if event.terminal
            && state
                .terminal_by_request
                .insert(event.request_id.clone(), event.sequence)
                .is_some()
        {
            return Err(ReplayError::CorruptLog(format!(
                "line {line_number} records a second terminal event for {}",
                event.request_id
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

/// Names are stored only as digests, so any non-empty name is a valid one.
fn validate_name(value: &str) -> Result<(), ReplayError> {
    if value.is_empty() {
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
