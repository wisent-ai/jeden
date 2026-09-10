mod workspace;
pub(crate) use workspace::migrate as migrate_workspace;

use super::model::*;
use serde_json::Value;
use std::fs::{self, File, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};

pub(crate) const STATE_FILE: &str = "completion.json";
const LOCK_FILE: &str = "completion.lock";

fn regular_file_or_absent(path: &Path) -> Result<(), String> {
    match fs::symlink_metadata(path) {
        Ok(metadata) if metadata.is_file() && !metadata.file_type().is_symlink() => Ok(()),
        Ok(_) => Err(format!(
            "completion state path is not a regular file: {}",
            path.display()
        )),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(format!("cannot inspect {}: {error}", path.display())),
    }
}

fn session_directory(dir: &Path) -> Result<(), String> {
    let metadata = fs::symlink_metadata(dir)
        .map_err(|error| format!("cannot inspect session {}: {error}", dir.display()))?;
    if !metadata.is_dir() || metadata.file_type().is_symlink() || !dir.join("state.json").is_file()
    {
        return Err(format!("not a durable Jeden session: {}", dir.display()));
    }
    Ok(())
}

pub(crate) fn read(dir: &Path) -> Result<CompletionState, String> {
    session_directory(dir)?;
    let file = dir.join(STATE_FILE);
    regular_file_or_absent(&file)?;
    let state = match fs::read(&file) {
        Ok(bytes) => serde_json::from_slice(&bytes)
            .map_err(|error| format!("invalid completion state {}: {error}", file.display()))?,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => legacy_state(dir)?,
        Err(error) => {
            return Err(format!(
                "cannot read completion state {}: {error}",
                file.display()
            ))
        }
    };
    state.validate()?;
    Ok(state)
}

pub(crate) fn update<T>(
    dir: &Path,
    expected_revision: Option<u64>,
    change: impl FnOnce(&mut CompletionState) -> Result<T, String>,
) -> Result<(T, CompletionState), String> {
    session_directory(dir)?;
    let lock_path = dir.join(LOCK_FILE);
    regular_file_or_absent(&lock_path)?;
    let lock = OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .open(&lock_path)
        .map_err(|error| {
            format!(
                "cannot open completion lock {}: {error}",
                lock_path.display()
            )
        })?;
    lock.lock()
        .map_err(|error| format!("cannot lock completion state: {error}"))?;
    let mut state = read(dir)?;
    if let Some(expected) = expected_revision {
        if state.revision != expected {
            return Err(format!(
                "completion state changed: expected revision {expected}, found {}",
                state.revision
            ));
        }
    }
    let output = change(&mut state)?;
    state.revision = state
        .revision
        .checked_add(u64::from(true))
        .ok_or("completion revision overflow")?;
    state.validate()?;
    write_atomic(&dir.join(STATE_FILE), &state)?;
    let legacy = dir.join("artifacts/todo.json");
    if legacy.exists() {
        regular_file_or_absent(&legacy)?;
        fs::remove_file(&legacy).map_err(|error| {
            format!("completion state saved but legacy todo retirement failed: {error}")
        })?;
    }
    Ok((output, state))
}

fn write_atomic(path: &Path, state: &CompletionState) -> Result<(), String> {
    regular_file_or_absent(path)?;
    let parent = path
        .parent()
        .ok_or("completion state has no parent directory")?;
    let staging = parent.join(format!(".completion-{}.new", uuid::Uuid::new_v4()));
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let result = (|| {
        let mut file = options.open(&staging).map_err(|error| error.to_string())?;
        serde_json::to_writer_pretty(&mut file, state).map_err(|error| error.to_string())?;
        file.write_all(b"\n").map_err(|error| error.to_string())?;
        file.sync_all().map_err(|error| error.to_string())?;
        fs::rename(&staging, path).map_err(|error| error.to_string())?;
        #[cfg(unix)]
        File::open(parent)
            .and_then(|directory| directory.sync_all())
            .map_err(|error| error.to_string())?;
        Ok(())
    })();
    if result.is_err() {
        let _ = fs::remove_file(&staging);
    }
    result.map_err(|error: String| format!("cannot persist {}: {error}", path.display()))
}

/// Old todo claims are imported as unverified work, never as completed work.
/// The old file is retired only after its replacement is durably committed.
fn legacy_state(dir: &Path) -> Result<CompletionState, String> {
    let mut state = legacy_requests(dir)?;
    let path = dir.join("artifacts/todo.json");
    regular_file_or_absent(&path)?;
    let legacy: Value = match fs::read(&path) {
        Ok(bytes) => serde_json::from_slice(&bytes)
            .map_err(|error| format!("invalid legacy todo {}: {error}", path.display()))?,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(state),
        Err(error) => {
            return Err(format!(
                "cannot read legacy todo {}: {error}",
                path.display()
            ))
        }
    };
    let phases = legacy
        .get("phases")
        .and_then(Value::as_array)
        .ok_or("legacy todo has no phases array")?;
    let request_id = "legacy-todo".to_string();
    for phase in phases {
        let name = phase
            .get("phase")
            .and_then(Value::as_str)
            .unwrap_or("Tasks");
        let items = phase
            .get("items")
            .and_then(Value::as_array)
            .ok_or("legacy todo phase has no items array")?;
        for (index, item) in items.iter().enumerate() {
            let text = item
                .get("text")
                .and_then(Value::as_str)
                .filter(|text| !text.trim().is_empty())
                .ok_or("legacy todo task has no text")?;
            state.tasks.push(WorkTask {
                id: format!("legacy-{}-{index}", state.tasks.len()),
                request_id: request_id.clone(),
                phase: name.to_string(),
                text: text.to_string(),
                criteria: vec![text.to_string()],
                kind: TaskKind::Work,
                origin: TaskOrigin::User,
                status: TaskStatus::VerificationRequested,
                reason: Some(
                    "Legacy todo status was an agent claim, not independent verification.".into(),
                ),
                verification: None,
            });
        }
    }
    if !state.tasks.is_empty() {
        let session_state: Value = serde_json::from_slice(
            &fs::read(dir.join("state.json"))
                .map_err(|error| format!("cannot read legacy task workspace: {error}"))?,
        )
        .map_err(|error| format!("invalid legacy task workspace: {error}"))?;
        let cwd = session_state
            .get("cwd")
            .and_then(Value::as_str)
            .ok_or("legacy task session has no workspace")?;
        state.requests.push(WorkRequest {
            id: request_id,
            prompt: "Verify and finish every retained legacy task; inspect prior results before repeating any operation.".into(),
            cwd: cwd.to_string(),
            paused: false,
            captured_at: crate::agent::now_stamp(),
            planned: true,
            coverage_verified: false,
        });
    }
    Ok(state)
}

fn legacy_requests(dir: &Path) -> Result<CompletionState, String> {
    let mut state = CompletionState::default();
    let transcript = dir.join("transcript.jsonl");
    regular_file_or_absent(&transcript)?;
    if !transcript.exists() {
        return Ok(state);
    }
    let cwd = super::cli::workspace(dir)?;
    let ledger = crate::cli::sessions::ledger_v2::store::read_events(dir)?;
    for event in ledger.events {
        if event.payload.kind() != "user" {
            continue;
        }
        let data = event.payload.data();
        if data.get("modelOnly").and_then(Value::as_bool) == Some(true)
            || data.get("completionManaged").and_then(Value::as_bool) == Some(false)
        {
            continue;
        }
        let prompt = data
            .get("rawTask")
            .or_else(|| data.get("task"))
            .or_else(|| data.get("prompt"))
            .or_else(|| data.get("content"))
            .and_then(Value::as_str)
            .or_else(|| data.as_str())
            .filter(|prompt| !prompt.trim().is_empty())
            .ok_or_else(|| {
                format!(
                    "legacy user event {} has no recorded request",
                    event.event_id
                )
            })?;
        state.requests.push(WorkRequest {
            id: event.event_id,
            prompt: prompt.to_string(),
            cwd: data
                .get("cwd")
                .and_then(Value::as_str)
                .map(str::to_string)
                .unwrap_or_else(|| cwd.display().to_string()),
            paused: false,
            captured_at: event.timestamp,
            planned: false,
            coverage_verified: false,
        });
    }
    Ok(state)
}

pub(crate) fn inherit(source: &Path, destination: &Path) -> Result<CompletionState, String> {
    let inherited = read(source)?;
    let (_, state) = update(destination, None, |state| {
        if !state.requests.is_empty() || !state.tasks.is_empty() {
            return Err("cannot replace an existing session's completion obligations".into());
        }
        state.requests = inherited.requests;
        state.tasks = inherited.tasks;
        state.blocker = inherited.blocker;
        Ok(())
    })?;
    Ok(state)
}

pub(crate) fn session_from_artifacts(path: &Path) -> Result<PathBuf, String> {
    if path.file_name().and_then(|name| name.to_str()) != Some("artifacts") {
        return Err("todo requires the active session artifact directory".into());
    }
    let session = path.parent().ok_or("todo has no owning session")?;
    session_directory(session)?;
    Ok(session.to_path_buf())
}
