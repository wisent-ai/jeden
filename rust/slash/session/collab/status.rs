//! Saying what a collaboration is actually doing right now, in one line a
//! person can read.
//!
//! Split out of `slash/session/collab.rs`, which had grown past the module
//! line cap.

use crate::slash::session::collab::relay::read_collab_events;
use serde_json::Value;
use std::path::Path;

pub(super) fn collab_descriptor(entry: &Value) -> String {
    if let Some(file) = entry.get("relayFile").and_then(Value::as_str) {
        format!("durable file relay: {}", file)
    } else {
        "off".into()
    }
}

pub(super) fn collab_role_status(role: &str, entry: &Value, view: bool) -> String {
    if entry.is_null() {
        return format!("Collab {role}: off.");
    }
    let relay_file = entry.get("relayFile").and_then(Value::as_str).unwrap_or("");
    let events = read_collab_events(Path::new(relay_file));
    let latest = events
        .last()
        .and_then(|event| event.get("type"))
        .and_then(Value::as_str)
        .unwrap_or("none");
    let mut lines = vec![
        format!("Collab {role}: {}", collab_descriptor(entry)),
        format!(
            "Relay URL: {}",
            entry.get("relayUrl").and_then(Value::as_str).unwrap_or("")
        ),
        format!("Events: {}", events.len()),
        format!("Latest event: {}", latest),
    ];
    if view {
        if events.is_empty() {
            lines.push("Event log is empty.".into());
        } else {
            lines.push("Event log:".into());
            // A number-free running ordinal: start from the u64 default and step
            // by the unit derived from `true`. `Value` renders via `Display`
            // (infallible), so no serialize recovery path is needed.
            let mut ordinal = u64::default();
            for event in &events {
                ordinal += u64::from(true);
                lines.push(format!("{}. {}", ordinal, event));
            }
        }
    }
    lines.join("\n")
}

/// Status for an HTTP-backed collab role. Shows the relay base + room + live
/// event count fetched from the relay (opaque blobs; contents stay encrypted).
pub(super) fn collab_http_role_status(role: &str, entry: &Value) -> String {
    let base = entry.get("relayBase").and_then(Value::as_str).unwrap_or("");
    let room = entry.get("room").and_then(Value::as_str).unwrap_or("");
    let count =
        crate::collab::relay_get(base, room, usize::default()).map(|(events, _)| events.len());
    let events_line = match count {
        Ok(n) => format!("Events: {} (encrypted)", n),
        Err(e) => format!("Events: unavailable ({})", e),
    };
    [
        format!("Collab {role}: HTTP relay {}", base),
        format!("Room: {}", room),
        events_line,
        "Payloads are end-to-end encrypted; the relay never sees plaintext or the key.".to_string(),
    ]
    .join("\n")
}

pub(super) fn picker_role_detail(role: &str, entry: &Value) -> String {
    if entry.is_null() {
        return format!("{role}: off");
    }
    match entry.get("backend").and_then(Value::as_str) {
        Some("http") => format!(
            "{role}: HTTP relay {} room {}; encryption key is not persisted",
            entry
                .get("relayBase")
                .and_then(Value::as_str)
                .unwrap_or("not recorded"),
            entry
                .get("room")
                .and_then(Value::as_str)
                .unwrap_or("not recorded")
        ),
        _ => format!("{role}: {}", collab_descriptor(entry)),
    }
}
