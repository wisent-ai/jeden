use crate::cli::sessions::ledger_v2::store::read_events;
use serde_json::{json, Value};
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

pub(crate) fn failed(result: &Value) -> bool {
    result.get("ok").and_then(Value::as_bool) == Some(false)
        || result.get("success").and_then(Value::as_bool) == Some(false)
        || result.get("failed").and_then(Value::as_bool) == Some(true)
        || result.get("error").is_some_and(|error| !error.is_null())
        || result.get("exitCode").and_then(Value::as_i64).is_some_and(|code| code != i64::default())
        || result.get("exit_code").and_then(Value::as_i64).is_some_and(|code| code != i64::default())
        || result.get("status").and_then(Value::as_u64).is_some_and(|status| status >= super::super::constants::HTTP_ERROR_STATUS)
}

pub(crate) fn receipts(session: &Path) -> Result<BTreeMap<String, Value>, String> {
    lineage_receipts(session, &mut BTreeSet::new())
}

fn lineage_receipts(session: &Path, visited: &mut BTreeSet<PathBuf>) -> Result<BTreeMap<String, Value>, String> {
    let session = std::fs::canonicalize(session).map_err(|error| error.to_string())?;
    if !visited.insert(session.clone()) {
        return Err("evidence session lineage contains a cycle".into());
    }
    let ledger = read_events(&session)?;
    let mut receipts = BTreeMap::new();
    let mut input = Value::Null;
    for event in ledger.events {
        let data = event.payload.data();
        match event.payload.kind() {
            "lineage" => {
                let parent = data.get("parentSession").and_then(Value::as_str)
                    .ok_or("evidence lineage has no parent session")?;
                receipts.extend(lineage_receipts(Path::new(parent), visited)?);
            }
            "tool_call" => input = data.get("input").cloned().unwrap_or(Value::Null),
            "tool_result" => {
                let result = data.get("result").cloned().unwrap_or(Value::Null);
                receipts.insert(event.event_id.clone(), json!({
                    "sessionPath": session,
                    "eventId": event.event_id,
                    "tool": data.get("tool"),
                    "input": input,
                    "result": result,
                    "failed": failed(&result),
                    "recoveredTruncatedTail": ledger.recovered_truncated_tail,
                    "checksum": event.checksum,
                }));
                input = Value::Null;
            }
            _ => {}
        }
    }
    Ok(receipts)
}

pub(crate) fn review_evidence(session: &Path) -> Result<Value, String> {
    let entries = receipts(session)?;
    Ok(Value::Array(entries.into_values().map(|mut entry| {
        let result = entry["result"].to_string();
        let preview: String = result.chars().take(super::super::constants::EVIDENCE_PREVIEW_CHARS).collect();
        entry.as_object_mut().expect("receipt object").remove("result");
        entry["resultPreview"] = json!(preview);
        entry
    }).collect()))
}

pub(crate) fn inspect(cwd: &Path, input: &Value) -> Result<Value, String> {
    let session = input.get("sessionPath").and_then(Value::as_str)
        .ok_or("task_evidence requires sessionPath")?;
    let event_id = input.get("eventId").and_then(Value::as_str)
        .ok_or("task_evidence requires eventId")?;
    let session = crate::cli::sessions::session_dir_for(session);
    let state_bytes = std::fs::read(session.join("state.json"))
        .map_err(|error| format!("cannot read evidence session: {error}"))?;
    let state: Value = serde_json::from_slice(&state_bytes).map_err(|error| error.to_string())?;
    let source_cwd = state.get("cwd").and_then(Value::as_str)
        .ok_or("evidence session has no workspace")?;
    if std::fs::canonicalize(source_cwd).map_err(|error| error.to_string())?
        != std::fs::canonicalize(cwd).map_err(|error| error.to_string())? {
        return Err("task evidence belongs to a different workspace".into());
    }
    receipts(&session)?.remove(event_id)
        .ok_or_else(|| format!("no recorded tool evidence with id {event_id}"))
}
