//! Building the list of sessions on disk an operator chooses from, and the
//! preview beside each one.
//!
//! Split out of `slash/modes/session.rs`, which had grown past the module line
//! cap.

use super::dates::relative_age;
use crate::tui::PickerItem;
use serde::Deserialize;
use std::fs;
use std::io::BufRead;
use std::path::{Path, PathBuf};

/// The fields of a `user` event that may carry what the operator asked, in
/// the order they are read.
#[derive(Default, Deserialize)]
#[serde(default)]
struct UserEventData {
    task: serde_json::Value,
    content: serde_json::Value,
    text: serde_json::Value,
}

/// First user task/message in the session transcript, reading only until the
/// first `user` event line. Handles both V2 (`payload.type`) and legacy
/// (`type`) transcript lines. A session with no transcript has no task; a
/// transcript that cannot be read is an `Err` with the file and cause, shown
/// in the preview rather than taken for a session that asked nothing.
fn first_user_task(session_dir: &Path) -> Result<Option<String>, String> {
    let transcript = session_dir.join("transcript.jsonl");
    let file = match fs::File::open(&transcript) {
        Ok(file) => file,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(format!("{}: {error}", transcript.display())),
    };
    first_task_in(std::io::BufReader::new(file), &transcript)
}

fn first_task_in(reader: impl BufRead, transcript: &Path) -> Result<Option<String>, String> {
    for line in reader.lines() {
        let line = line.map_err(|error| format!("{}: {error}", transcript.display()))?;
        // A line that is not JSON is a damaged transcript, shown as such in the
        // picker with the line itself, never passed over on the way to the first task.
        let value = serde_json::from_str::<serde_json::Value>(&line).map_err(|error| {
            format!("{}: a line is not a transcript event ({error}): {line}", transcript.display())
        })?;
        // A V2 line carries its event under payload; a legacy line is the
        // event itself.
        let payload = match value.get("payload") {
            Some(payload) => payload,
            None => &value,
        };
        if payload.get("type").and_then(serde_json::Value::as_str) != Some("user") {
            continue;
        }
        let Some(text) = user_text(payload, transcript)? else {
            return Ok(None);
        };
        let collapsed = text.split_whitespace().collect::<Vec<_>>().join(" ");
        if !collapsed.is_empty() {
            return Ok(Some(collapsed));
        }
    }
    Ok(None)
}

/// The text a `user` event's data carries, read in the order its fields are
/// declared; none when the event carries no data, and an error naming the
/// transcript when its data is not the shape a user event has.
fn user_text(payload: &serde_json::Value, transcript: &Path) -> Result<Option<String>, String> {
    let Some(data) = payload.get("data") else {
        return Ok(None);
    };
    let said = UserEventData::deserialize(data).map_err(|error| {
        format!("{}: a user event's data is not a user event: {error}", transcript.display())
    })?;
    let found = [&said.task, &said.content, &said.text]
        .into_iter()
        .find_map(serde_json::Value::as_str)
        .map(str::to_string);
    Ok(found)
}

/// The row for something the listing could not read: disabled, its cause in
/// the detail, so a broken session is shown instead of vanishing or posing
/// as an unnamed one.
fn unreadable_item(label: String, cause: String) -> PickerItem {
    PickerItem::action(label, "")
        .detail(cause)
        .badge("error")
        .disabled(true)
}

pub(super) fn session_items(session_root: &Path) -> Vec<PickerItem> {
    let mut entries = Vec::new();
    let mut unreadable = Vec::new();
    let read_dir = match fs::read_dir(session_root) {
        Ok(read_dir) => read_dir,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Vec::new(),
        Err(error) => {
            return vec![unreadable_item(
                "Sessions cannot be listed".into(),
                format!("{}: {error}", session_root.display()),
            )]
        }
    };
    {
        for entry in read_dir {
            let entry = match entry {
                Ok(entry) => entry,
                Err(error) => {
                    unreadable.push(unreadable_item(
                        "A session cannot be listed".into(),
                        format!("{}: {error}", session_root.display()),
                    ));
                    continue;
                }
            };
            let path = entry.path();
            match std::fs::metadata(&path) {
                Ok(metadata) if metadata.is_dir() => {}
                Ok(_not_session) => continue,
                Err(error) => {
                    unreadable.push(unreadable_item(
                        entry.file_name().to_string_lossy().to_string(),
                        format!("{}: {error}", path.display()),
                    ));
                    continue;
                }
            }
            let id = entry.file_name().to_string_lossy().to_string();
            let metadata = match crate::slash::common::read_json_value(&path.join("state.json")) {
                Ok(metadata) => metadata,
                Err(cause) => {
                    unreadable.push(unreadable_item(id, cause));
                    continue;
                }
            };
            // A session nobody named is listed by its id; a workspace or a
            // start it does not record is left out of its detail.
            let text = |key: &str| {
                metadata
                    .get(key)
                    .and_then(serde_json::Value::as_str)
                    .filter(|value| !value.trim().is_empty())
                    .map(str::to_string)
            };
            let name = match text("name") {
                Some(name) => name,
                None => id.clone(),
            };
            entries.push((path, id, name, text("cwd"), text("startedAt")));
        }
    }
    let mut items = Vec::new();
    for (path, id, name, workspace, started) in &entries {
        let mut parts: Vec<String> = workspace.iter().chain(started.iter()).cloned().collect();
        if let Some(started) = started.as_deref() {
            parts.push(relative_age(started));
        }
        match first_user_task(path) {
            Ok(Some(task)) => parts.push(task),
            Ok(None) => {}
            Err(cause) => parts.push(format!("transcript unreadable: {cause}")),
        }
        let detail = parts.join(" · ");
        items.push(
            PickerItem::action(name.clone(), format!("/resume {}", id))
                .detail(detail)
                .badge("SESSION"),
        );
    }
    items.sort_by(|left, right| right.label.cmp(&left.label));
    items.extend(unreadable);
    items
}

/// List every session directory under `session_root`, one per line; every
/// session is listed. A root or an entry that cannot be read is an error
/// naming the root and the cause, so `/resume` does not answer success with
/// a refusal in its text.
pub(super) fn list_sessions(session_root: &Path) -> Result<String, String> {
    let refused = |error: std::io::Error| format!("Sessions cannot be listed: {}: {error}", session_root.display());
    let entries = match fs::read_dir(session_root) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok("No sessions found.".into()),
        Err(error) => return Err(refused(error)),
    };
    let mut rows = Vec::new();
    for entry in entries {
        rows.push(entry.map_err(&refused)?.file_name().to_string_lossy().to_string());
    }
    if rows.is_empty() {
        return Ok("No sessions found.".into());
    }
    Ok(rows.join("\n"))
}

pub(super) fn session_path(session_root: &Path, id_or_path: &str) -> PathBuf {
    if id_or_path.contains('/') {
        PathBuf::from(id_or_path)
    } else {
        session_root.join(id_or_path)
    }
}
