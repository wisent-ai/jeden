//! Appending to a session's ledger through one lock.
//!
//! Split out of `cli/reports/sessions.rs`, which had grown past the module
//! line cap.

use super::ledger_v2::{
    self, CheckpointPayloadV2, RewindPayloadV2, SessionEventV2, SessionPayloadV2,
};
use super::{LedgerEntry, LEDGER_APPEND_LOCK, SESSION_LEDGER_VERSION};
use serde_json::{json, Value};
use std::path::Path;
use crate::cli::reports::sessions::pending::unresolved_pending_claim;
use crate::cli::reports::sessions::replay::parse_transcript;

impl LedgerEntry {
    pub(super) fn from_event(event: &SessionEventV2) -> Self {
        Self {
            version: SESSION_LEDGER_VERSION,
            id: event.event_id.clone(),
            parent_id: event.parent_id.clone(),
            ts: event.timestamp.clone(),
            kind: event.payload.kind().to_owned(),
            data: event.payload.data().clone(),
        }
    }
}
pub(crate) fn append_ledger_entry(
    dir: &Path,
    ts: String,
    kind: &str,
    data: Value,
) -> Result<LedgerEntry, String> {
    let _guard = LEDGER_APPEND_LOCK
        .lock()
        .map_err(|_| "session ledger append lock poisoned")?;
    append_ledger_entry_unlocked(dir, ts, kind, data)
}

pub(super) fn append_ledger_entry_unlocked(
    dir: &Path,
    ts: String,
    kind: &str,
    data: Value,
) -> Result<LedgerEntry, String> {
    let payload = SessionPayloadV2::from_legacy(kind, data)?;
    ledger_v2::store::append(dir, ts, payload).map(|event| LedgerEntry::from_event(&event))
}

pub(crate) fn append_checkpoint_entry(
    dir: &Path,
    ts: String,
    label: Option<String>,
    messages: &[Value],
) -> Result<LedgerEntry, String> {
    let _guard = LEDGER_APPEND_LOCK
        .lock()
        .map_err(|_| "session ledger append lock poisoned")?;
    let payload = SessionPayloadV2::checkpoint(CheckpointPayloadV2 {
        label,
        messages: messages.to_vec(),
    })?;
    ledger_v2::store::append(dir, ts, payload).map(|event| LedgerEntry::from_event(&event))
}

pub(crate) fn append_rewind_entry(
    dir: &Path,
    ts: String,
    checkpoint_id: &str,
) -> Result<(LedgerEntry, Vec<Value>), String> {
    let _guard = LEDGER_APPEND_LOCK
        .lock()
        .map_err(|_| "session ledger append lock poisoned")?;
    let ledger = parse_transcript(dir)?;
    if unresolved_pending_claim(&ledger.active_entries).is_some() {
        return Err("cannot rewind while a pending action claim is unresolved".into());
    }
    let global = ledger
        .events
        .iter()
        .find(|event| event.event_id == checkpoint_id);
    if !ledger
        .active_entries
        .iter()
        .any(|entry| entry.id == checkpoint_id)
    {
        return match global {
            Some(event) if !matches!(event.payload, SessionPayloadV2::Checkpoint(_)) => {
                Err(format!("event {checkpoint_id} is not a checkpoint"))
            }
            Some(_) => Err(format!(
                "checkpoint {checkpoint_id} is not an ancestor of active leaf {:?}",
                ledger.active_leaf
            )),
            None => Err(format!("checkpoint not found: {checkpoint_id}")),
        };
    }
    let checkpoint = global.ok_or_else(|| format!("checkpoint not found: {checkpoint_id}"))?;
    if !matches!(checkpoint.payload, SessionPayloadV2::Checkpoint(_)) {
        return Err(format!("event {checkpoint_id} is not a checkpoint"));
    }
    let checkpoint_data = checkpoint
        .payload
        .checkpoint_data()
        .map_err(|error| format!("checkpoint {checkpoint_id} has invalid payload: {error}"))?;
    let from_leaf_id = ledger
        .active_leaf
        .clone()
        .ok_or("cannot rewind an empty session ledger")?;
    let payload = SessionPayloadV2::rewind(RewindPayloadV2 {
        checkpoint_id: checkpoint_id.to_owned(),
        from_leaf_id,
    })?;
    let event =
        ledger_v2::store::append_with_parent(dir, ts, payload, Some(checkpoint_id.to_owned()))?;
    Ok((LedgerEntry::from_event(&event), checkpoint_data.messages))
}

pub(crate) fn session_active_leaf(dir: &Path) -> Result<Option<String>, String> {
    ledger_v2::store::reconcile_active_leaf(dir)
}

pub(super) fn export_event(event: &SessionEventV2) -> Value {
    let mut value = serde_json::to_value(event).unwrap_or(Value::Null);
    if let Some(object) = value.as_object_mut() {
        object.insert("version".into(), json!(event.schema_version));
        object.insert("id".into(), json!(event.event_id));
        object.insert("parentId".into(), json!(event.parent_id));
        object.insert("ts".into(), json!(event.timestamp));
        object.insert("type".into(), json!(event.payload.kind()));
        object.insert("data".into(), event.payload.data().clone());
    }
    value
}
