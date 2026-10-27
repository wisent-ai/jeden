//! Putting a transcript on disk so a machine that loses power mid-write does
//! not lose the history.
//!
//! Split out of `session/store.rs`, which had grown past the module line cap.

use super::super::event::SessionEventV2;
use super::TRANSCRIPT_FILE;
use serde_json::Value;
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::Path;

pub(super) fn rewrite_v2(dir: &Path, events: &[SessionEventV2]) -> Result<(), String> {
    let path = dir.join(TRANSCRIPT_FILE);
    let temp = dir.join(format!(".{TRANSCRIPT_FILE}.migrate-{}", std::process::id()));
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&temp)
        .map_err(|error| {
            format!(
                "cannot create transcript migration file {}: {error}",
                temp.display()
            )
        })?;
    for event in events {
        let mut encoded = serde_json::to_vec(event).map_err(|error| {
            format!(
                "cannot encode event {} for {}: {error}",
                event.event_id,
                path.display()
            )
        })?;
        encoded.push(b'\n');
        file.write_all(&encoded).map_err(|error| {
            format!(
                "cannot write transcript migration file {}: {error}",
                temp.display()
            )
        })?;
    }
    file.sync_all().map_err(|error| {
        format!(
            "cannot sync transcript migration file {}: {error}",
            temp.display()
        )
    })?;
    fs::rename(&temp, &path).map_err(|error| {
        format!(
            "cannot publish transcript migration {} as {}: {error}",
            temp.display(),
            path.display()
        )
    })?;
    sync_directory(dir)
}

pub(super) fn append_event_line(dir: &Path, event: &SessionEventV2) -> Result<(), String> {
    let path = dir.join(TRANSCRIPT_FILE);
    let mut encoded = serde_json::to_vec(event).map_err(|error| {
        format!(
            "cannot encode event {} for {}: {error}",
            event.event_id,
            path.display()
        )
    })?;
    encoded.push(b'\n');
    let mut file = OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)
        .map_err(|error| {
            format!(
                "cannot open transcript {} for append: {error}",
                path.display()
            )
        })?;
    file.write_all(&encoded).map_err(|error| {
        format!(
            "cannot append event {} to {}: {error}",
            event.event_id,
            path.display()
        )
    })?;
    file.sync_data().map_err(|error| {
        format!(
            "cannot sync appended transcript {}: {error}",
            path.display()
        )
    })
}

pub(super) fn read_session_id(dir: &Path) -> Result<String, String> {
    let path = dir.join("state.json");
    let value: Value = serde_json::from_slice(
        &fs::read(&path).map_err(|e| format!("cannot read {}: {}", path.display(), e))?,
    )
    .map_err(|e| format!("invalid {}: {}", path.display(), e))?;
    value
        .get("id")
        .and_then(Value::as_str)
        .map(str::to_owned)
        .or_else(|| {
            dir.file_name()
                .map(|name| name.to_string_lossy().into_owned())
        })
        .ok_or_else(|| format!("{} has no session id", path.display()))
}

pub(super) fn mirror_active_leaf(dir: &Path, active_leaf: Option<&str>) -> Result<(), String> {
    let path = dir.join("state.json");
    let mut state: Value = serde_json::from_slice(
        &fs::read(&path).map_err(|error| format!("cannot read {}: {}", path.display(), error))?,
    )
    .map_err(|error| format!("invalid {}: {}", path.display(), error))?;
    if state.get("activeLeaf").and_then(Value::as_str) == active_leaf
        && (active_leaf.is_some() || state.get("activeLeaf") == Some(&Value::Null))
    {
        return Ok(());
    }
    let object = state
        .as_object_mut()
        .ok_or_else(|| format!("invalid {}: expected object", path.display()))?;
    object.insert("activeLeaf".into(), Value::from(active_leaf));
    let suffix = uuid::Uuid::new_v4().simple();
    let temp = dir.join(format!(".state.json.active-leaf-{suffix}"));
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&temp)
        .map_err(|error| {
            format!(
                "cannot create active-leaf state file {}: {error}",
                temp.display()
            )
        })?;
    let mut encoded = serde_json::to_vec_pretty(&state).map_err(|error| {
        format!(
            "cannot encode active-leaf state for {}: {error}",
            path.display()
        )
    })?;
    encoded.push(b'\n');
    file.write_all(&encoded).map_err(|error| {
        format!(
            "cannot write active-leaf state file {}: {error}",
            temp.display()
        )
    })?;
    file.sync_all().map_err(|error| {
        format!(
            "cannot sync active-leaf state file {}: {error}",
            temp.display()
        )
    })?;
    fs::rename(&temp, &path).map_err(|error| {
        format!(
            "cannot publish active-leaf state {} as {}: {error}",
            temp.display(),
            path.display()
        )
    })?;
    sync_directory(dir)
}

/// A new event id: its timestamp and a random UUID, so two events written in
/// the same instant never share one.
pub(super) fn fresh_event_id(timestamp: &str) -> String {
    format!("event-{timestamp}-{}", uuid::Uuid::new_v4().simple())
}

#[cfg(unix)]
pub(super) fn sync_directory(dir: &Path) -> Result<(), String> {
    let directory = std::fs::File::open(dir).map_err(|error| {
        format!(
            "cannot open session directory {} for sync: {error}",
            dir.display()
        )
    })?;
    directory
        .sync_all()
        .map_err(|error| format!("cannot sync session directory {}: {error}", dir.display()))
}
#[cfg(not(unix))]
pub(super) fn sync_directory(_dir: &Path) -> Result<(), String> {
    Ok(())
}
