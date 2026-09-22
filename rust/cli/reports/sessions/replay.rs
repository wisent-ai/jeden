//! Rebuilding the model-visible conversation from a recorded session.
//!
//! Split out of `cli/reports/sessions.rs`, which had grown past the module
//! line cap.

use super::ledger_v2;
use super::{LedgerEntry, SessionLedger};
use crate::cli::reports::sessions::ledger_v2::event::payload::CheckpointPayloadV2;
use serde_json::{json, Value};
use std::path::Path;

pub(super) fn parse_transcript(dir: &Path) -> Result<SessionLedger, String> {
    let ledger = ledger_v2::store::read_events(dir)?;
    let active_leaf = ledger.events.last().map(|event| event.event_id.clone());
    let active_entries = ledger_v2::store::active_lineage(&ledger.events, active_leaf.as_deref())?
        .into_iter()
        .map(LedgerEntry::from_event)
        .collect();
    let entries = ledger.events.iter().map(LedgerEntry::from_event).collect();
    Ok(SessionLedger {
        entries,
        active_entries,
        active_leaf,
        recovered_truncated_tail: ledger.recovered_truncated_tail,
        events: ledger.events,
    })
}

/// Faithfully rebuild the model-visible message window. Snapshot entries are
/// durable cut points; older legacy events are replayed through adapters.
pub(crate) fn session_conversation_turns(dir: &Path) -> Result<Vec<Value>, String> {
    replay_entries(parse_transcript(dir)?.active_entries)
}

pub(crate) fn list_checkpoint_entries(dir: &Path) -> Result<String, String> {
    let ledger = parse_transcript(dir)?;
    let mut rows = Vec::new();
    for entry in ledger
        .active_entries
        .iter()
        .filter(|entry| entry.kind == "checkpoint")
    {
        let checkpoint: CheckpointPayloadV2 =
            serde_json::from_value(entry.data.clone()).map_err(|error| {
                format!(
                    "ledger entry {} has invalid checkpoint: {}",
                    entry.id, error
                )
            })?;
        rows.push(match checkpoint.label {
            Some(label) => format!("{}\t{}", entry.id, label),
            None => entry.id.clone(),
        });
    }
    Ok(if rows.is_empty() {
        "No checkpoints on the active session lineage.".into()
    } else {
        rows.join("\n")
    })
}

pub(super) fn replay_entries(entries: Vec<LedgerEntry>) -> Result<Vec<Value>, String> {
    let mut messages = Vec::new();
    for entry in entries {
        let data = entry.data;
        match entry.kind.as_str() {
            "context_snapshot" => {
                messages = data
                    .get("messages")
                    .and_then(Value::as_array)
                    .cloned()
                    .ok_or_else(|| {
                        format!("ledger entry {} has invalid context snapshot", entry.id)
                    })?;
            }
            "checkpoint" => {
                let checkpoint: CheckpointPayloadV2 =
                    serde_json::from_value(data).map_err(|error| {
                        format!(
                            "ledger entry {} has invalid checkpoint: {}",
                            entry.id, error
                        )
                    })?;
                messages = checkpoint.messages;
            }
            "user" => {
                if let Some(task) = data.get("task").and_then(Value::as_str) {
                    messages.push(json!({ "role": "user", "content": task }));
                }
            }
            "assistant_raw" => {
                if let Some(content) = data.get("content").and_then(Value::as_str) {
                    messages.push(json!({ "role": "assistant", "content": content }));
                }
            }
            "final" | "assistant_message" => {
                if let Some(text) = data.get("text").and_then(Value::as_str) {
                    if let Some(last) = messages.last_mut().filter(|message| {
                        message.get("role").and_then(Value::as_str) == Some("assistant")
                    }) {
                        last["content"] = json!(text);
                    } else {
                        messages.push(json!({ "role": "assistant", "content": text }));
                    }
                }
            }
            "tool_result" => {
                if data.get("replayPending").and_then(Value::as_bool) == Some(true) {
                    continue;
                }
                if let Some(content) = data.get("replayMessage").and_then(Value::as_str) {
                    messages.push(json!({ "role": "user", "content": content }));
                } else if let Some(result) = data.get("result") {
                    messages.push(json!({ "role": "user", "content": crate::tool_runtime::format_tool_result(result) }));
                }
            }
            "compaction" => {
                let before = data.get("before").and_then(Value::as_u64).unwrap_or(0);
                let summary = data.get("summary").and_then(Value::as_str).unwrap_or("");
                messages = vec![
                    json!({ "role": "system", "content": format!("Prior conversation summary (compacted from {} messages):\n{}", before, summary), "_jedenNeedsBaseSystem": true }),
                ];
            }
            "auto_continue" => {
                if let Some(prompt) = data.get("prompt").and_then(Value::as_str) {
                    messages.push(json!({ "role": "user", "content": prompt }));
                }
            }
            // The request for a delivery report went to the model as a user
            // message; a resumed conversation must carry the same exchange.
            "contract_violation"
                if data.get("outcome").and_then(Value::as_str) == Some("requested") =>
            {
                messages.push(json!({
                    "role": "user",
                    "content": data.get("prompt").and_then(Value::as_str)
                        .unwrap_or(crate::agent::task_contract::REPAIR_INSTRUCTION),
                }));
            }
            _ => {}
        }
    }
    Ok(messages)
}
