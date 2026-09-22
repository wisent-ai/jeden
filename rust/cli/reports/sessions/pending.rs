//! Actions that were prepared but not yet performed.
//!
//! Split out of `cli/reports/sessions.rs`, which had grown past the module
//! line cap.

use super::ledger::append_ledger_entry_unlocked;
use super::{parse_transcript, LedgerEntry, LEDGER_APPEND_LOCK, NEXT_ENTRY_ID};
use serde_json::json;
use std::fs;
use std::path::Path;
use std::sync::atomic::Ordering;

const MAX_PENDING_PAYLOAD_BYTES: usize = 8 * 1024 * 1024;

#[derive(Debug, Clone)]
pub(crate) struct PendingActionCreate {
    pub(crate) kind: String,
    pub(crate) target: String,
    pub(crate) expected_sha256: String,
    pub(crate) payload: Vec<u8>,
    pub(crate) preview: String,
    pub(crate) ttl_seconds: u64,
}

#[derive(Debug, Clone)]
pub(crate) struct PendingActionClaim {
    pub(crate) id: String,
    pub(crate) kind: String,
    pub(crate) target: String,
    pub(crate) expected_sha256: String,
    pub(crate) payload: Vec<u8>,
}

fn pending_session_dir(artifact_dir: &Path) -> Result<&Path, String> {
    artifact_dir
        .parent()
        .ok_or_else(|| "pending registry artifact directory has no session parent".into())
}

fn operation_ready(
    operation: &crate::tool_runtime::runtime_ops::OperationContext<'_>,
) -> Result<(), String> {
    if operation.cancellation().is_cancelled() {
        return Err("pending action cancelled".into());
    }
    Ok(())
}

pub(crate) fn create_pending_action(
    artifact_dir: &Path,
    operation: &crate::tool_runtime::runtime_ops::OperationContext<'_>,
    create: PendingActionCreate,
) -> Result<String, String> {
    operation_ready(operation)?;
    if create.payload.len() > MAX_PENDING_PAYLOAD_BYTES {
        return Err("pending action payload exceeds limit".into());
    }
    let session_dir = pending_session_dir(artifact_dir)?;
    let now = now_epoch_seconds();
    let id = format!(
        "pending-{}-{:016x}",
        now,
        NEXT_ENTRY_ID.fetch_add(1, Ordering::Relaxed)
    );
    let dir = artifact_dir.join("pending-actions");
    fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let payload_path = dir.join(format!("{}.payload", id));
    fs::write(&payload_path, &create.payload).map_err(|e| e.to_string())?;
    append_ledger_entry(
        session_dir,
        now.to_string(),
        "pending_preview",
        json!({
            "pendingId": id,
            "kind": create.kind,
            "target": create.target,
            "expectedSha256": create.expected_sha256,
            "payloadPath": payload_path,
            "preview": create.preview,
            "createdAt": now,
            "expiresAt": now.saturating_add(create.ttl_seconds),
        }),
    )?;
    Ok(id)
}

fn pending_terminal(entries: &[LedgerEntry], id: &str) -> bool {
    entries.iter().any(|entry| {
        matches!(
            entry.kind.as_str(),
            "pending_claim" | "pending_apply" | "pending_discard" | "pending_expire"
        ) && entry.data.get("pendingId").and_then(Value::as_str) == Some(id)
    })
}

fn pending_resolved(entries: &[LedgerEntry], id: &str) -> bool {
    entries.iter().any(|entry| {
        matches!(
            entry.kind.as_str(),
            "pending_apply" | "pending_discard" | "pending_expire"
        ) && entry.data.get("pendingId").and_then(Value::as_str) == Some(id)
    })
}

fn unresolved_pending_claim(entries: &[LedgerEntry]) -> Option<&str> {
    entries.iter().rev().find_map(|entry| {
        if entry.kind != "pending_claim" {
            return None;
        }
        let id = entry.data.get("pendingId").and_then(Value::as_str)?;
        (!pending_resolved(entries, id)).then_some(id)
    })
}

pub(crate) fn claim_pending_action(
    artifact_dir: &Path,
    operation: &crate::tool_runtime::runtime_ops::OperationContext<'_>,
    id: &str,
) -> Result<PendingActionClaim, String> {
    operation_ready(operation)?;
    let session_dir = pending_session_dir(artifact_dir)?;
    let _guard = LEDGER_APPEND_LOCK
        .lock()
        .map_err(|_| "session ledger append lock poisoned")?;
    let ledger = parse_transcript(session_dir)?;
    if pending_terminal(&ledger.entries, id) {
        return Err(format!("pending action is already resolved: {id}"));
    }
    let entry = ledger
        .active_entries
        .iter()
        .rev()
        .find(|entry| {
            entry.kind == "pending_preview"
                && entry.data.get("pendingId").and_then(Value::as_str) == Some(id)
        })
        .cloned();
    let Some(entry) = entry else {
        if ledger.entries.iter().any(|entry| {
            entry.kind == "pending_preview"
                && entry.data.get("pendingId").and_then(Value::as_str) == Some(id)
        }) {
            return Err(format!(
                "pending action is not on the active session lineage: {id}"
            ));
        }
        return Err(format!("pending action not found: {id}"));
    };
    let expires_at = entry
        .data
        .get("expiresAt")
        .and_then(Value::as_u64)
        .ok_or_else(|| format!("pending action {id} has invalid expiry"))?;
    let now = now_epoch_seconds();
    if now >= expires_at {
        append_ledger_entry_unlocked(
            session_dir,
            now.to_string(),
            "pending_expire",
            json!({ "pendingId": id }),
        )?;
        return Err(format!("pending action expired: {id}"));
    }
    let payload_path = artifact_dir
        .join("pending-actions")
        .join(format!("{id}.payload"));
    let payload = fs::read(&payload_path)
        .map_err(|error| format!("cannot read pending action {id} payload: {error}"))?;
    operation_ready(operation)?;
    append_ledger_entry_unlocked(
        session_dir,
        now.to_string(),
        "pending_claim",
        json!({ "pendingId": id }),
    )?;
    let kind = entry
        .data
        .get("kind")
        .and_then(Value::as_str)
        .ok_or_else(|| format!("pending action {id} has invalid kind"))?;
    let target = entry
        .data
        .get("target")
        .and_then(Value::as_str)
        .ok_or_else(|| format!("pending action {id} has invalid target"))?;
    let expected_sha256 = entry
        .data
        .get("expectedSha256")
        .and_then(Value::as_str)
        .ok_or_else(|| format!("pending action {id} has invalid revision"))?;
    Ok(PendingActionClaim {
        id: id.to_owned(),
        kind: kind.to_owned(),
        target: target.to_owned(),
        expected_sha256: expected_sha256.to_owned(),
        payload,
    })
}

pub(crate) fn complete_pending_action(artifact_dir: &Path, id: &str) -> Result<(), String> {
    let session_dir = pending_session_dir(artifact_dir)?;
    let _guard = LEDGER_APPEND_LOCK
        .lock()
        .map_err(|_| "session ledger append lock poisoned")?;
    let ledger = parse_transcript(session_dir)?;
    if pending_resolved(&ledger.entries, id) {
        return Err(format!("pending action is already resolved: {id}"));
    }
    if !ledger.active_entries.iter().any(|entry| {
        entry.kind == "pending_claim"
            && entry.data.get("pendingId").and_then(Value::as_str) == Some(id)
    }) {
        return Err(format!(
            "pending action claim is not on the active session lineage: {id}"
        ));
    }
    append_ledger_entry_unlocked(
        session_dir,
        now_epoch_seconds().to_string(),
        "pending_apply",
        json!({ "pendingId": id }),
    )?;
    remove_pending_payload(artifact_dir, id)
}

pub(crate) fn discard_pending_action(
    artifact_dir: &Path,
    operation: &crate::tool_runtime::runtime_ops::OperationContext<'_>,
    id: &str,
) -> Result<(), String> {
    operation_ready(operation)?;
    let session_dir = pending_session_dir(artifact_dir)?;
    let _guard = LEDGER_APPEND_LOCK
        .lock()
        .map_err(|_| "session ledger append lock poisoned")?;
    let ledger = parse_transcript(session_dir)?;
    if pending_terminal(&ledger.entries, id) {
        return Err(format!("pending action is already resolved: {id}"));
    }
    if !ledger.active_entries.iter().any(|entry| {
        entry.kind == "pending_preview"
            && entry.data.get("pendingId").and_then(Value::as_str) == Some(id)
    }) {
        if ledger.entries.iter().any(|entry| {
            entry.kind == "pending_preview"
                && entry.data.get("pendingId").and_then(Value::as_str) == Some(id)
        }) {
            return Err(format!(
                "pending action is not on the active session lineage: {id}"
            ));
        }
        return Err(format!("pending action not found: {id}"));
    }
    append_ledger_entry_unlocked(
        session_dir,
        now_epoch_seconds().to_string(),
        "pending_discard",
        json!({ "pendingId": id }),
    )?;
    remove_pending_payload(artifact_dir, id)
}

fn remove_pending_payload(artifact_dir: &Path, id: &str) -> Result<(), String> {
    match fs::remove_file(
        artifact_dir
            .join("pending-actions")
            .join(format!("{}.payload", id)),
    ) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error.to_string()),
    }
}

fn now_epoch_seconds() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}
