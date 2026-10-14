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
/// was hosted or joined, and the next save replaced the one that was.
pub(super) fn read_collab_state(cwd: &Path) -> Result<Value, String> {
    let file = collab_state_path(cwd);
    let state = read_json_value(&file)?;
    if !state.is_object() {
        return Err(format!(
            "{} does not hold a JSON object, so the collaboration state is unknown",
            file.display()
        ));
    }
    Ok(state)
}

/// The relay backend a saved host or guest entry names in `backend`. Only the
/// HTTP relay is told apart; any other entry is on a durable file relay.
#[derive(Deserialize, PartialEq)]
#[serde(rename_all = "lowercase")]
enum CollabBackend {
    Http,
}

/// Whether a saved host or guest entry is on the HTTP relay.
pub(super) fn on_http_relay(entry: &Value) -> bool {
    entry
        .get("backend")
        .cloned()
        .and_then(|backend| serde_json::from_value::<CollabBackend>(backend).ok())
        == Some(CollabBackend::Http)
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
        let url = Url::parse(text).map_err(|e| e.to_string())?;
        return url
            .to_file_path()
            .map_err(|_| "Invalid file relay URL".to_string());
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
        fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }
    let line = serde_json::to_string(&json!({ "ts": now_text()?, "type": event_type, "cwd": cwd }))
        .map_err(|e| e.to_string())?;
    let mut file = fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
        .map_err(|e| e.to_string())?;
    writeln!(file, "{}", line).map_err(|e| e.to_string())
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

pub(super) fn save_collab_state(cwd: &Path, state: &Value) -> Result<PathBuf, String> {
    let file = collab_state_path(cwd);
    let host = state.get("host").cloned().unwrap_or(Value::Null);
    let guest = state.get("guest").cloned().unwrap_or(Value::Null);
    write_json_value(
        &file,
        &json!({ "updatedAt": now_text()?, "host": host, "guest": guest }),
    )?;
    Ok(file)
}

/// Encrypt a collab event under `key` and POST it to the HTTP relay. The relay
/// only ever sees the ciphertext; the key never leaves this process. The key is
/// taken as a slice and converted to the fixed-size array the cipher requires,
/// so no array-length literal is written here.
pub(super) fn post_collab_http(
    base: &str,
    room: &str,
    key: &[u8],
    write_token: &str,
    event_type: &str,
    cwd: &Path,
) -> Result<(), String> {
    let key_array: &[u8; 32] = key
        .try_into()
        .map_err(|_| "collab key has an unexpected length".to_string())?;
    let frame = crate::collab::ProtocolFrame::new(
        "jeden-slash",
        crate::collab::CollabRole::Full,
        crate::collab::FrameKind::State {
            value: json!({ "event": event_type, "ts": now_text()?, "cwd": cwd }),
        },
    )?;
    let blob = crate::collab::seal_frame(key_array, &frame)?;
    crate::collab::relay_post_authorized(base, room, &blob, Some(write_token))?;
    Ok(())
}
