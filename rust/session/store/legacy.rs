//! Reading a transcript written by an older build, and carrying it forward
//! without losing what it recorded.
//!
//! Split out of `session/store.rs`, which had grown past the module line cap.

use super::super::event::{SessionEventV2, SessionPayloadV2, SESSION_EVENT_SCHEMA_VERSION};
use serde::Deserialize;
use serde_json::Value;

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

pub(super) fn migrate_legacy_value(
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
