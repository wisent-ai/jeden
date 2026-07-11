use super::event::{SessionEventV2, SessionPayloadV2, SESSION_EVENT_SCHEMA_VERSION};
use super::outbox::{OutboxConsumer, OutboxItem};
use rand::{distributions::Alphanumeric, Rng};
use serde::Deserialize;
use serde_json::Value;
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::Path;

const TRANSCRIPT_FILE: &str = "transcript.jsonl";

#[derive(Debug)]
pub(crate) struct ReadEvents {
    pub(crate) events: Vec<SessionEventV2>,
    pub(crate) recovered_truncated_tail: bool,
    pub(crate) contained_legacy: bool,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct LegacyLedgerEntry {
    version: u32,
    id: String,
    parent_id: Option<String>,
    ts: String,
    #[serde(rename = "type")]
    kind: String,
    data: Value,
}

pub(crate) fn append(
    dir: &Path,
    timestamp: String,
    payload: SessionPayloadV2,
) -> Result<SessionEventV2, String> {
    let mut ledger = read_events(dir)?;
    if ledger.recovered_truncated_tail {
        return Err(format!("cannot append {}: transcript has a recovered truncated tail; resume into a child session", dir.display()));
    }
    if ledger.contained_legacy {
        rewrite_v2(dir, &ledger.events)?;
        ledger.contained_legacy = false;
    }
    let session_id = read_session_id(dir)?;
    let parent_id = ledger.events.last().map(|event| event.event_id.clone());
    let sequence = ledger.events.len() as u64 + 1;
    let event_id = fresh_event_id(&timestamp);
    let correlation_id = ledger
        .events
        .last()
        .map(|event| event.correlation_id.clone())
        .unwrap_or_else(|| event_id.clone());
    let mut event = SessionEventV2 {
        event_id: event_id.clone(),
        session_id,
        parent_id: parent_id.clone(),
        sequence,
        timestamp,
        causation_id: parent_id,
        correlation_id,
        schema_version: SESSION_EVENT_SCHEMA_VERSION,
        payload,
        outbox: OutboxConsumer::ALL
            .into_iter()
            .map(|consumer| OutboxItem::pending(consumer, &event_id))
            .collect(),
        checksum: String::new(),
    };
    event.seal()?;
    append_event_line(dir, &event)?;
    Ok(event)
}

pub(crate) fn read_events(dir: &Path) -> Result<ReadEvents, String> {
    let path = dir.join(TRANSCRIPT_FILE);
    let bytes = match fs::read(&path) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Vec::new(),
        Err(error) => return Err(format!("cannot read {}: {}", path.display(), error)),
    };
    if bytes.is_empty() {
        return Ok(ReadEvents {
            events: Vec::new(),
            recovered_truncated_tail: false,
            contained_legacy: false,
        });
    }
    let terminated = bytes.last() == Some(&b'\n');
    let chunks = bytes.split(|byte| *byte == b'\n').collect::<Vec<_>>();
    let logical_len = chunks.len().saturating_sub(usize::from(terminated));
    let session_id = read_session_id(dir)?;
    let mut events = Vec::with_capacity(logical_len);
    let mut recovered_truncated_tail = false;
    let mut contained_legacy = false;

    for (index, raw) in chunks.into_iter().take(logical_len).enumerate() {
        let line_number = index + 1;
        let recoverable_tail = !terminated && line_number == logical_len;
        let value: Value = match serde_json::from_slice(raw) {
            Ok(value) => value,
            Err(_) if recoverable_tail => {
                recovered_truncated_tail = true;
                break;
            }
            Err(error) => {
                return Err(format!(
                    "{}:{} is malformed JSON: {}",
                    path.display(),
                    line_number,
                    error
                ))
            }
        };
        let event = if value.get("schemaVersion").is_some() {
            let event: SessionEventV2 = serde_json::from_value(value).map_err(|e| {
                format!(
                    "{}:{} is not a valid V2 event: {}",
                    path.display(),
                    line_number,
                    e
                )
            })?;
            event
                .verify()
                .map_err(|e| format!("{}:{} {}", path.display(), line_number, e))?;
            event
        } else {
            contained_legacy = true;
            migrate_legacy_value(value, &session_id, line_number as u64, events.last())
                .map_err(|e| format!("{}:{} {}", path.display(), line_number, e))?
        };
        validate_next(&path, line_number, &events, &event, &session_id)?;
        events.push(event);
    }
    Ok(ReadEvents {
        events,
        recovered_truncated_tail,
        contained_legacy,
    })
}

fn migrate_legacy_value(
    value: Value,
    session_id: &str,
    sequence: u64,
    previous: Option<&SessionEventV2>,
) -> Result<SessionEventV2, String> {
    let (id, parent_id, timestamp, kind, data) = if value.get("version").is_some() {
        let legacy: LegacyLedgerEntry =
            serde_json::from_value(value).map_err(|e| format!("invalid V1 ledger entry: {e}"))?;
        if legacy.version != 1 {
            return Err(format!(
                "unsupported legacy ledger version {}",
                legacy.version
            ));
        }
        (
            legacy.id,
            legacy.parent_id,
            legacy.ts,
            legacy.kind,
            legacy.data,
        )
    } else {
        let timestamp = value
            .get("ts")
            .and_then(Value::as_str)
            .ok_or("legacy event has no string ts")?
            .to_owned();
        let kind = value
            .get("type")
            .and_then(Value::as_str)
            .ok_or("legacy event has no string type")?
            .to_owned();
        let data = value
            .get("data")
            .cloned()
            .ok_or("legacy event has no data")?;
        (
            format!("legacy-{sequence}"),
            previous.map(|event| event.event_id.clone()),
            timestamp,
            kind,
            data,
        )
    };
    let payload = SessionPayloadV2::from_legacy(&kind, data)?;
    let correlation_id = previous
        .map(|event| event.correlation_id.clone())
        .unwrap_or_else(|| id.clone());
    let mut event = SessionEventV2 {
        event_id: id.clone(),
        session_id: session_id.to_owned(),
        parent_id: parent_id.clone(),
        sequence,
        timestamp,
        causation_id: parent_id,
        correlation_id,
        schema_version: SESSION_EVENT_SCHEMA_VERSION,
        payload,
        outbox: OutboxConsumer::ALL
            .into_iter()
            .map(|consumer| OutboxItem::pending(consumer, &id))
            .collect(),
        checksum: String::new(),
    };
    event.seal()?;
    Ok(event)
}

fn validate_next(
    path: &Path,
    line: usize,
    prior: &[SessionEventV2],
    event: &SessionEventV2,
    session_id: &str,
) -> Result<(), String> {
    let expected_sequence = prior.len() as u64 + 1;
    let expected_parent = prior.last().map(|entry| entry.event_id.clone());
    if event.sequence != expected_sequence {
        return Err(format!(
            "{}:{} breaks ledger sequence: {}, expected {}",
            path.display(),
            line,
            event.sequence,
            expected_sequence
        ));
    }
    if event.parent_id != expected_parent {
        return Err(format!(
            "{}:{} breaks ledger lineage: parent {:?}, active leaf {:?}",
            path.display(),
            line,
            event.parent_id,
            expected_parent
        ));
    }
    if event.session_id != session_id {
        return Err(format!(
            "{}:{} belongs to session {}, expected {}",
            path.display(),
            line,
            event.session_id,
            session_id
        ));
    }
    if event.causation_id != event.parent_id {
        return Err(format!("{}:{} has invalid causation", path.display(), line));
    }
    if event.correlation_id.is_empty() {
        return Err(format!(
            "{}:{} has empty correlation id",
            path.display(),
            line
        ));
    }
    let valid_outbox = event.outbox.len() == OutboxConsumer::ALL.len()
        && OutboxConsumer::ALL.iter().all(|consumer| {
            let expected = OutboxItem::pending(*consumer, &event.event_id);
            event.outbox.iter().any(|item| item == &expected)
        });
    if !valid_outbox {
        return Err(format!(
            "{}:{} has invalid transactional outbox seeds",
            path.display(),
            line
        ));
    }
    Ok(())
}

fn rewrite_v2(dir: &Path, events: &[SessionEventV2]) -> Result<(), String> {
    let path = dir.join(TRANSCRIPT_FILE);
    let temp = dir.join(format!(".{TRANSCRIPT_FILE}.migrate-{}", std::process::id()));
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&temp)
        .map_err(|e| e.to_string())?;
    for event in events {
        let mut encoded = serde_json::to_vec(event).map_err(|e| e.to_string())?;
        encoded.push(b'\n');
        file.write_all(&encoded).map_err(|e| e.to_string())?;
    }
    file.sync_all().map_err(|e| e.to_string())?;
    fs::rename(&temp, &path).map_err(|e| e.to_string())?;
    sync_directory(dir)
}

fn append_event_line(dir: &Path, event: &SessionEventV2) -> Result<(), String> {
    let mut encoded = serde_json::to_vec(event).map_err(|e| e.to_string())?;
    encoded.push(b'\n');
    let mut file = OpenOptions::new()
        .create(true)
        .append(true)
        .open(dir.join(TRANSCRIPT_FILE))
        .map_err(|e| e.to_string())?;
    file.write_all(&encoded).map_err(|e| e.to_string())?;
    file.sync_data().map_err(|e| e.to_string())
}

fn read_session_id(dir: &Path) -> Result<String, String> {
    let path = dir.join("state.json");
    let value: Value = serde_json::from_slice(
        &fs::read(&path).map_err(|e| format!("cannot read {}: {}", path.display(), e))?,
    )
    .map_err(|e| format!("invalid {}: {}", path.display(), e))?;
    value
        .get("id")
        .and_then(Value::as_str)
        .map(str::to_owned)
        .or_else(|| {
            dir.file_name()
                .map(|name| name.to_string_lossy().into_owned())
        })
        .ok_or_else(|| format!("{} has no session id", path.display()))
}

fn fresh_event_id(timestamp: &str) -> String {
    let suffix: String = rand::thread_rng()
        .sample_iter(&Alphanumeric)
        .take(12)
        .map(char::from)
        .collect();
    format!("event-{timestamp}-{suffix}")
}

#[cfg(unix)]
fn sync_directory(dir: &Path) -> Result<(), String> {
    std::fs::File::open(dir)
        .and_then(|file| file.sync_all())
        .map_err(|e| e.to_string())
}
#[cfg(not(unix))]
fn sync_directory(_dir: &Path) -> Result<(), String> {
    Ok(())
}
