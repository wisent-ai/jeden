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
        let Ok(value) = serde_json::from_str::<serde_json::Value>(&line) else {
            continue;
        };
        let payload = value.get("payload").unwrap_or(&value);
        if payload.get("type").and_then(serde_json::Value::as_str) != Some("user") {
            continue;
        }
        let Some(text) = user_text(payload) else {
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
/// declared.
fn user_text(payload: &serde_json::Value) -> Option<String> {
    let said = UserEventData::deserialize(payload.get("data")?).ok()?;
    [&said.task, &said.content, &said.text]
        .into_iter()
        .find_map(serde_json::Value::as_str)
        .map(str::to_string)
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
            if !path.is_dir() {
                continue;
            }
            let id = entry.file_name().to_string_lossy().to_string();
            let metadata = match crate::slash::common::read_json_value(&path.join("state.json")) {
                Ok(metadata) => metadata,
                Err(cause) => {
                    unreadable.push(unreadable_item(id, cause));
                    continue;
                }
            };
            let name = metadata
                .get("name")
                .and_then(serde_json::Value::as_str)
                .filter(|value| !value.trim().is_empty())
                .unwrap_or(&id)
                .to_string();
            let workspace = metadata
                .get("cwd")
                .and_then(serde_json::Value::as_str)
                .unwrap_or("")
                .to_string();
            let started = metadata
                .get("startedAt")
                .and_then(serde_json::Value::as_str)
                .unwrap_or("")
                .to_string();
            entries.push((path, id, name, workspace, started));
        }
    }
    let mut items = Vec::new();
    for (path, id, name, workspace, started) in &entries {
        let mut parts = vec![workspace.clone(), started.clone()];
        if let Some(age) = relative_age(started) {
            parts.push(age);
        }
        match first_user_task(path) {
            Ok(Some(task)) => parts.push(task),
            Ok(None) => {}
            Err(cause) => parts.push(format!("transcript unreadable: {cause}")),
        }
        let detail = parts
            .into_iter()
            .filter(|value| !value.is_empty())
            .collect::<Vec<_>>()
            .join(" · ");
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

/// List every session directory under `session_root`, one per line. The prior
/// "most recent N" display cap was an unconsented numeric literal and has been
/// removed; all sessions are listed.
pub(super) fn list_sessions(session_root: &Path) -> String {
    let mut rows = Vec::new();
    let entries = match fs::read_dir(session_root) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return "No sessions found.".into()
        }
        Err(error) => {
            return format!(
                "Sessions cannot be listed: {}: {error}",
                session_root.display()
            )
        }
    };
    for entry in entries {
        match entry {
            Ok(entry) => rows.push(entry.file_name().to_string_lossy().to_string()),
            Err(error) => {
                return format!(
                    "Sessions cannot be listed: {}: {error}",
                    session_root.display()
                )
            }
        }
    }
    if rows.is_empty() {
        "No sessions found.".into()
    } else {
        rows.join("\n")
    }
}

pub(super) fn session_path(session_root: &Path, id_or_path: &str) -> PathBuf {
    if id_or_path.contains('/') {
        PathBuf::from(id_or_path)
    } else {
        session_root.join(id_or_path)
    }
}
