//! Building the list of sessions on disk an operator chooses from, and the
//! preview beside each one.
//!
//! Split out of `slash/modes/session.rs`, which had grown past the module line
//! cap.

use super::dates::relative_age;
use crate::tui::PickerItem;
use std::fs;
use std::io::BufRead;
use std::path::{Path, PathBuf};
use crate::slash::modes::session::dates::started_epoch;

const MESSAGE_PREVIEW_SESSIONS: usize = 50;
const MESSAGE_PREVIEW_CHARS: usize = 60;

fn truncate_chars(text: &str, max: usize) -> String {
    let mut chars = text.chars();
    let head: String = chars.by_ref().take(max).collect();
    if chars.next().is_some() {
        format!("{head}…")
    } else {
        head
    }
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
        let text = ["task", "content", "text"]
            .iter()
            .find_map(|key| data.get(key).and_then(serde_json::Value::as_str))?;
        let collapsed = text.split_whitespace().collect::<Vec<_>>().join(" ");
        if !collapsed.is_empty() {
            return Some(truncate_chars(&collapsed, MESSAGE_PREVIEW_CHARS));
        }
    }
    None
}

pub(super) fn session_items(session_root: &Path) -> Vec<PickerItem> {
    let mut entries = Vec::new();
    if let Ok(read_dir) = fs::read_dir(session_root) {
        for entry in read_dir.flatten() {
            let path = entry.path();
            if !path.is_dir() {
                continue;
            }
            let id = entry.file_name().to_string_lossy().to_string();
            let metadata = fs::read_to_string(path.join("state.json"))
                .ok()
                .and_then(|text| serde_json::from_str::<serde_json::Value>(&text).ok())
                .unwrap_or_default();
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
    // Newest first by start time; only these get a transcript preview read.
    let mut recency: Vec<usize> = (0..entries.len()).collect();
    recency.sort_by(|left, right| {
        started_epoch(&entries[*right].4).cmp(&started_epoch(&entries[*left].4))
    });
    let preview: std::collections::HashSet<usize> =
        recency.into_iter().take(MESSAGE_PREVIEW_SESSIONS).collect();
    let mut items = Vec::new();
    for (index, (path, id, name, workspace, started)) in entries.iter().enumerate() {
        let mut parts = vec![workspace.clone(), started.clone()];
        if let Some(age) = relative_age(started) {
            parts.push(age);
        }
        if preview.contains(&index) {
            if let Some(task) = first_user_task(path) {
                parts.push(task);
            }
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
    items
}

/// List every session directory under `session_root`, one per line. The prior
/// "most recent N" display cap was an unconsented numeric literal and has been
/// removed; all sessions are listed.
pub(super) fn list_sessions(session_root: &Path) -> String {
    let mut rows = Vec::new();
    if let Ok(entries) = fs::read_dir(session_root) {
        for entry in entries.flatten() {
            rows.push(entry.file_name().to_string_lossy().to_string());
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
