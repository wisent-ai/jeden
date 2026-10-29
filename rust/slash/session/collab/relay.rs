//! Where a shared session's events are written, and how they get there.
//!
//! Split out of `slash/session/collab.rs`, which had grown past the module
//! line cap.

use crate::slash::common::{now_text, read_json_value, write_json_value};
use serde::Deserialize;
use serde_json::{json, Value};
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use url::Url;

pub(super) fn collab_state_path(cwd: &Path) -> PathBuf {
    cwd.join(".jeden/collab.json")
}

/// The collaboration state; an empty object when none was saved yet. A state
/// file that cannot be read, does not parse, or whose root is not an object
/// is refused naming the file: read as empty, a broken state said no session
/// was hosted or joined, and the next save replaced the one that was. A host
/// or guest entry must be off (`null`) or name its relay backend, `http` or
/// `file`, as `/collab start` and `/join` write it; one that does not is
/// refused with serde's account of it, so no reader takes a damaged HTTP
/// entry for a relay file that does not exist.
pub(super) fn read_collab_state(cwd: &Path) -> Result<Value, String> {
    let file = collab_state_path(cwd);
    let state = read_json_value(&file)?;
    if !state.is_object() {
        return Err(format!(
            "{} does not hold a JSON object, so the collaboration state is unknown",
            file.display()
        ));
    }
    serde_json::from_value::<SavedRoles>(state.clone()).map_err(|error| {
        format!(
            "{} holds a host or guest that names no relay backend this build knows: {error}",
            file.display()
        )
    })?;
    Ok(state)
}

/// The roles a saved state holds, each off or naming its relay backend.
#[derive(Deserialize)]
struct SavedRoles {
    #[serde(rename = "host")]
    _host: Option<SavedRole>,
    #[serde(rename = "guest")]
    _guest: Option<SavedRole>,
}

#[derive(Deserialize)]
struct SavedRole {
    #[serde(rename = "backend")]
    _backend: CollabBackend,
}

/// The relay backend a saved host or guest entry names in `backend`: the
/// HTTP relay or a durable file relay, as `/collab start` and `/join` write
/// it.
#[derive(Deserialize, PartialEq)]
#[serde(rename_all = "lowercase")]
enum CollabBackend {
    Http,
    File,
}

/// Whether a saved host or guest entry is on the HTTP relay. Every entry
/// `read_collab_state` answers names `http` or `file`.
pub(super) fn on_http_relay(entry: &Value) -> bool {
    entry.get("backend").and_then(Value::as_str) == Some("http")
}

/// Whether a `/collab start` or `/join` target is a network relay URL rather
/// than a relay file: a URL whose scheme has a known network port. A `file:`
/// URL or a path names a durable file relay.
pub(super) fn is_network_relay(target: &str) -> bool {
    Url::parse(target).is_ok_and(|url| url.port_or_known_default().is_some())
}

pub(crate) fn collab_default_relay(cwd: &Path) -> PathBuf {
    cwd.join(".jeden/collab-relay.jsonl")
}

pub(super) fn collab_path(cwd: &Path, target: &str) -> Result<PathBuf, String> {
    let text = target.trim();
    if text.starts_with("http://") || text.starts_with("https://") {
        return Err("Rust collab currently supports durable file relays only; HTTP relay support remains JS-only.".into());
    }
    if text.starts_with("file://") {
        let url =
            Url::parse(text).map_err(|error| format!("the relay {text} is not a URL: {error}"))?;
        // `to_file_path` answers `()` on refusal: a file URL with a host other
        // than localhost, or one this platform cannot express as a path.
        return url.to_file_path().map_err(|()| {
            format!("the relay {text} names no local file (a host other than localhost, or a path this platform cannot express)")
        });
    }
    if text.is_empty() {
        return Ok(collab_default_relay(cwd));
    }
    let path = PathBuf::from(text);
    Ok(if path.is_absolute() {
        path
    } else {
        cwd.join(path)
    })
}

pub(super) fn append_collab_event(path: &Path, event_type: &str, cwd: &Path) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|error| {
            format!(
                "the relay directory {} cannot be created: {error}",
                parent.display()
            )
        })?;
    }
    let line = serde_json::to_string(&json!({ "ts": now_text()?, "type": event_type, "cwd": cwd }))
        .map_err(|error| format!("the {event_type} relay event cannot be encoded: {error}"))?;
    let mut file = fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
        .map_err(|error| {
            format!(
                "the relay {} cannot be opened for appending: {error}",
                path.display()
            )
        })?;
    writeln!(file, "{}", line).map_err(|error| {
        format!(
            "the {event_type} event cannot be appended to {}: {error}",
            path.display()
        )
    })
}

/// The events in the relay file; none when the file does not exist yet. A
/// file that cannot be read, or an event line that does not parse, is the
/// error, not a shorter log.
pub(crate) fn read_collab_events(path: &Path) -> Result<Vec<Value>, String> {
    let text = match fs::read_to_string(path) {
        Ok(text) => text,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => String::new(),
        Err(error) => return Err(format!("cannot read {}: {error}", path.display())),
    };
    text.lines()
        .map(|line| {
            serde_json::from_str::<Value>(line).map_err(|error| {
                format!(
                    "{} holds an event that is not valid JSON ({error}): {line}",
                    path.display()
                )
            })
        })
        .collect()
}

/// Write the host and guest entries of `state`; a role that is not in it is
/// written as none (`null`), which is how this file says a role is off.
pub(super) fn save_collab_state(cwd: &Path, state: &Value) -> Result<PathBuf, String> {
    let file = collab_state_path(cwd);
    let role = |name: &str| match state.get(name) {
        Some(entry) => entry.clone(),
        None => Value::Null,
    };
    write_json_value(
        &file,
        &json!({ "updatedAt": now_text()?, "host": role("host"), "guest": role("guest") }),
    )?;
    Ok(file)
}

/// Encrypt a collab event under `key` and POST it to the HTTP relay. The relay
/// only ever sees the ciphertext; the key never leaves this process.
pub(super) fn post_collab_http(
    base: &str,
    room: &str,
    key: &crate::collab::RoomKey,
    write_token: &str,
    event_type: &str,
    cwd: &Path,
) -> Result<(), String> {
    let frame = crate::collab::ProtocolFrame::new(
        "jeden-slash",
        crate::collab::CollabRole::Full,
        crate::collab::FrameKind::State {
            value: json!({ "event": event_type, "ts": now_text()?, "cwd": cwd }),
        },
    )?;
    let blob = crate::collab::seal_frame(key, &frame)?;
    crate::collab::relay_post_authorized(base, room, &blob, Some(write_token))?;
    Ok(())
}
