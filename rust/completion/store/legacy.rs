//! Completion state for a session written before `completion.json`: its old
//! todo claims and the requests its transcript recorded, imported as work
//! still to verify.

use super::regular_file_or_absent;
use crate::completion::model::*;
use serde_json::Value;
use std::fs;
use std::path::Path;

/// Old todo claims are imported as unverified work, never as completed work.
/// The old file is retired only after its replacement is durably committed.
pub(super) fn legacy_state(dir: &Path) -> Result<CompletionState, String> {
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
            .ok_or("legacy todo phase has no name")?;
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
                defect_of: None,
                origin: TaskOrigin::User,
                status: TaskStatus::VerificationRequested,
                reason: Some(
                    "Legacy todo status was an agent claim, not independent verification.".into(),
                ),
                verification: None,
                operator_request: None,
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
            captured_at: crate::agent::now_stamp()
                .map_err(|error| format!("the legacy task request cannot be dated: {error}"))?,
            planned: true,
            coverage_verified: false,
            estimate: None,
            completed_at: None,
        });
    }
    Ok(state)
}

fn legacy_requests(dir: &Path) -> Result<CompletionState, String> {
    let mut state = CompletionState::default();
    let transcript = dir.join("transcript.jsonl");
    regular_file_or_absent(&transcript)?;
    if !crate::tool_runtime::runtime_ops::platform::presence::present(&transcript)? {
        return Ok(state);
    }
    let cwd = crate::completion::cli::workspace(dir)?;
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
        // An event that names no workspace of its own ran in the session's.
        let event_cwd = match data.get("cwd").and_then(Value::as_str) {
            Some(named) => named.to_string(),
            None => cwd.display().to_string(),
        };
        state.requests.push(WorkRequest {
            id: event.event_id,
            prompt: prompt.to_string(),
            cwd: event_cwd,
            paused: false,
            captured_at: event.timestamp,
            planned: false,
            coverage_verified: false,
            estimate: None,
            completed_at: None,
        });
    }
    Ok(state)
}
