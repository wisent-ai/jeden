use serde_json::Value;
use std::fs;
use std::path::Path;

/// Only a never-executed import can be refreshed. Any native turn, captured
/// extra request, task planning, or child ledger makes replacement unsafe.
pub(super) fn verify_unadopted(root: &Path, destination: &Path) -> Result<(), String> {
    if destination.join("adopted").exists() {
        return Err("native session has been adopted; refusing import refresh".into());
    }
    let snapshot = crate::cli::sessions::read_session_value(&destination.display().to_string())?;
    let events = snapshot["events"].as_array().ok_or("native ledger has no events")?;
    if events.len() != 1 || events[0]["type"] != "context_snapshot"
        || events[0].pointer("/data/reason").and_then(Value::as_str) != Some("omp-import") {
        return Err("native session has been used; refusing import refresh".into());
    }
    let completion_path = destination.join("completion.json");
    if completion_path.exists() {
        let state: Value = serde_json::from_slice(&fs::read(&completion_path).map_err(|e| e.to_string())?)
            .map_err(|e| e.to_string())?;
        let source = super::source::read(&destination.join("artifacts/omp-source.jsonl"))?;
        let requests = state["requests"].as_array().ok_or("native requests are invalid")?;
        if requests.len() != source.pending.len()
            || requests.iter().zip(&source.pending).any(|(request, prompt)|
                request["prompt"].as_str() != Some(prompt.as_str()))
            || state["revision"].as_u64() != Some(requests.len() as u64) {
            return Err("native retained requests have changed; refusing import refresh".into());
        }
        if state["tasks"].as_array().is_some_and(|tasks| !tasks.is_empty())
            || state["requests"].as_array().is_some_and(|requests|
                requests.iter().any(|request| request["planned"] == true || request["paused"] == true)) {
            return Err("native retained work has changed; refusing import refresh".into());
        }
    }
    for entry in fs::read_dir(root).map_err(|e| e.to_string())? {
        let path = entry.map_err(|e| e.to_string())?.path().join("state.json");
        if !path.is_file() { continue; }
        let state: Value = serde_json::from_slice(&fs::read(&path).map_err(|e| e.to_string())?)
            .map_err(|e| format!("cannot verify native lineage {}: {e}", path.display()))?;
        if state.pointer("/lineage/parentSession").and_then(Value::as_str)
            == destination.to_str() {
            return Err(format!("native child {} already continued this import; refusing refresh", path.display()));
        }
    }
    Ok(())
}

pub(super) fn recover(root: &Path, destination: &Path, id: &str) -> Result<(), String> {
    let backup = root.join(format!(".previous-{id}"));
    if backup.exists() && !destination.exists() {
        fs::rename(&backup, destination).map_err(|e| format!("cannot recover interrupted import publication: {e}"))?;
    }
    Ok(())
}

pub(super) fn publish(root: &Path, staging: &Path, destination: &Path, id: &str) -> Result<(), String> {
    let backup = root.join(format!(".previous-{id}"));
    if destination.exists() {
        verify_unadopted(root, destination)?;
        if backup.exists() { fs::remove_dir_all(&backup).map_err(|e| e.to_string())?; }
        fs::rename(destination, &backup).map_err(|e| e.to_string())?;
    }
    if let Err(error) = fs::rename(staging, destination) {
        recover(root, destination, id)?;
        return Err(format!("cannot publish imported session: {error}"));
    }
    fs::File::open(root).and_then(|f| f.sync_all()).map_err(|e| e.to_string())?;
    if backup.exists() { fs::remove_dir_all(&backup).map_err(|e| e.to_string())?; }
    Ok(())
}
