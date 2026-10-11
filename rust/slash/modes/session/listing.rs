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
/// (`type`) transcript lines.
fn first_user_task(session_dir: &Path) -> Option<String> {
    let file = fs::File::open(session_dir.join("transcript.jsonl")).ok()?;
    for line in std::io::BufReader::new(file).lines() {
        let Ok(line) = line else { break };
        let Ok(value) = serde_json::from_str::<serde_json::Value>(&line) else {
            continue;
        };
        let payload = value.get("payload").unwrap_or(&value);
        if payload.get("type").and_then(serde_json::Value::as_str) != Some("user") {
            continue;
        }
        let data = payload.get("data")?;
        let said = UserEventData::deserialize(data).ok()?;
        let text = [&said.task, &said.content, &said.text]
            .into_iter()
            .find_map(serde_json::Value::as_str)?;
        let collapsed = text.split_whitespace().collect::<Vec<_>>().join(" ");
        if !collapsed.is_empty() {
            return Some(collapsed);
        }
    }
    None
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
        for entry in read_dir.flatten() {
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
        if let Some(task) = first_user_task(path) {
            parts.push(task);
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
    for entry in entries.flatten() {
        rows.push(entry.file_name().to_string_lossy().to_string());
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
