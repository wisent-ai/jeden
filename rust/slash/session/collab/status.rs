//! Saying what a collaboration is actually doing right now, in one line a
//! person can read.
//!
//! Split out of `slash/session/collab.rs`, which had grown past the module
//! line cap.

use crate::slash::session::collab::relay::read_collab_events;
use crate::tally::FIRST;
use serde_json::Value;
use std::path::Path;

/// A recorded text field of a collaboration entry, or what says it was not recorded.
fn recorded<'a>(entry: &'a Value, field: &str) -> Result<&'a str, String> {
    entry
        .get(field)
        .and_then(Value::as_str)
        .ok_or_else(|| format!("the collaboration state records no {field}"))
}

pub(super) fn collab_descriptor(entry: &Value) -> String {
    match entry.get("relayFile").and_then(Value::as_str) {
        Some(file) => format!("durable file relay: {file}"),
        None => "off".into(),
    }
}

pub(super) fn collab_role_status(role: &str, entry: &Value, view: bool) -> String {
    if entry.is_null() {
        return format!("Collab {role}: off.");
    }
    let head = format!("Collab {role}: {}", collab_descriptor(entry));
    let events = match recorded(entry, "relayFile")
        .and_then(|relay_file| read_collab_events(Path::new(relay_file)))
    {
        Ok(events) => events,
        Err(cause) => return [head, format!("Events: unavailable ({cause})")].join("\n"),
    };
    let latest = match events
        .last()
        .map(|event| event.get("type").and_then(Value::as_str))
    {
        Some(Some(kind)) => kind.to_owned(),
        Some(None) => "an event without a type".to_owned(),
        None => "none".to_owned(),
    };
    let relay_url = match recorded(entry, "relayUrl") {
        Ok(url) => url.to_owned(),
        Err(cause) => cause,
    };
    let mut lines = vec![
        head,
        format!("Relay URL: {relay_url}"),
        format!("Events: {}", events.len()),
        format!("Latest event: {latest}"),
    ];
    if view {
        if events.is_empty() {
            lines.push("Event log is empty.".into());
        } else {
            lines.push("Event log:".into());
            for (ordinal, event) in (FIRST..).zip(&events) {
                lines.push(format!("{ordinal}. {event}"));
            }
        }
    }
    lines.join("\n")
}

/// Status for an HTTP-backed collab role. Shows the relay base + room + live
/// event count fetched from the relay (opaque blobs; contents stay encrypted).
pub(super) fn collab_http_role_status(role: &str, entry: &Value) -> String {
    let (base, room) = match (recorded(entry, "relayBase"), recorded(entry, "room")) {
        (Ok(base), Ok(room)) => (base, room),
        (Err(cause), _) | (_, Err(cause)) => return format!("Collab {role}: HTTP relay ({cause})"),
    };
    let events_line = match crate::collab::relay_get(base, room) {
        Ok((events, _)) => format!("Events: {} (encrypted)", events.len()),
        Err(e) => format!("Events: unavailable ({e})"),
    };
    [
        format!("Collab {role}: HTTP relay {base}"),
        format!("Room: {room}"),
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
        Some("http") => match (recorded(entry, "relayBase"), recorded(entry, "room")) {
            (Ok(base), Ok(room)) => {
                format!("{role}: HTTP relay {base} room {room}; encryption key is not persisted")
            }
            (Err(cause), _) | (_, Err(cause)) => format!("{role}: HTTP relay ({cause})"),
        },
        _ => format!("{role}: {}", collab_descriptor(entry)),
    }
}
