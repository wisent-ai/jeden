//! Appending to a session ledger and reading it back, with the parts that
//! validate a line, migrate an older one and put bytes on disk beside it.

use super::event::{SessionEventV2, SessionPayloadV2, SESSION_EVENT_SCHEMA_VERSION};
use super::outbox::{OutboxConsumer, OutboxItem};
use serde_json::Value;
use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::Path;

mod files;
mod legacy;
mod validate;

use files::{
    append_event_line, fresh_event_id, mirror_active_leaf, read_session_id, rewrite_v2,
    sync_directory,
};
use legacy::migrate_legacy_value;
use validate::validate_next;

pub(super) const TRANSCRIPT_FILE: &str = "transcript.jsonl";

#[derive(Debug)]
pub(crate) struct ReadEvents {
    pub(crate) events: Vec<SessionEventV2>,
    pub(crate) recovered_truncated_tail: bool,
    pub(crate) contained_legacy: bool,
}

pub(crate) fn append(
    dir: &Path,
    timestamp: String,
    payload: SessionPayloadV2,
) -> Result<SessionEventV2, String> {
    append_with_parent(dir, timestamp, payload, None)
}

pub(crate) fn append_with_parent(
    dir: &Path,
    timestamp: String,
    payload: SessionPayloadV2,
    explicit_parent: Option<String>,
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
    let parent_id =
        explicit_parent.or_else(|| ledger.events.last().map(|event| event.event_id.clone()));
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
    validate_next(
        &dir.join(TRANSCRIPT_FILE),
        ledger.events.len() + 1,
        &ledger.events,
        &event,
        &event.session_id,
    )?;
    append_event_line(dir, &event)?;
    if let Err(error) = mirror_active_leaf(dir, Some(&event.event_id)) {
        return Err(format!(
            "event {} committed, but active leaf mirror update failed: {}",
            event.event_id, error
        ));
    }
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

pub(crate) fn active_lineage<'a>(
    events: &'a [SessionEventV2],
    leaf_id: Option<&str>,
) -> Result<Vec<&'a SessionEventV2>, String> {
    let by_id = events
        .iter()
        .map(|event| (event.event_id.as_str(), event))
        .collect::<HashMap<_, _>>();
    let mut lineage = Vec::new();
    let mut seen = HashSet::new();
    let mut cursor = leaf_id;
    while let Some(id) = cursor {
        if !seen.insert(id.to_owned()) {
            return Err(format!("session lineage contains a cycle at {id}"));
        }
        let event = by_id
            .get(id)
            .copied()
            .ok_or_else(|| format!("session lineage references missing event {id}"))?;
        lineage.push(event);
        cursor = event.parent_id.as_deref();
    }
    lineage.reverse();
    Ok(lineage)
}

pub(crate) fn reconcile_active_leaf(dir: &Path) -> Result<Option<String>, String> {
    let ledger = read_events(dir)?;
    let active_leaf = ledger.events.last().map(|event| event.event_id.clone());
    mirror_active_leaf(dir, active_leaf.as_deref())?;
    Ok(active_leaf)
}
