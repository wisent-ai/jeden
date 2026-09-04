//! Session CLI subcommands: list, show, export, artifacts, resume, recall.

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Mutex;

#[path = "../session/mod.rs"]
pub(crate) mod ledger_v2;
use ledger_v2::{CheckpointPayloadV2, RewindPayloadV2, SessionEventV2, SessionPayloadV2};

static LEDGER_APPEND_LOCK: Mutex<()> = Mutex::new(());
static NEXT_ENTRY_ID: AtomicU64 = AtomicU64::new(1);

pub(crate) const SESSION_LEDGER_VERSION: u32 = ledger_v2::SESSION_EVENT_SCHEMA_VERSION;

/// Compatibility adapter for memory and collaboration consumers. The durable
/// representation is exclusively `SessionEventV2`; new consumers use it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct LedgerEntry {
    pub(crate) version: u32,
    pub(crate) id: String,
    pub(crate) parent_id: Option<String>,
    pub(crate) ts: String,
    #[serde(rename = "type")]
    pub(crate) kind: String,
    pub(crate) data: Value,
}

impl LedgerEntry {
    fn from_event(event: &SessionEventV2) -> Self {
        Self {
            version: SESSION_LEDGER_VERSION,
            id: event.event_id.clone(),
            parent_id: event.parent_id.clone(),
            ts: event.timestamp.clone(),
            kind: event.payload.kind().to_owned(),
            data: event.payload.data().clone(),
        }
    }
}
pub(crate) fn append_ledger_entry(
    dir: &Path,
    ts: String,
    kind: &str,
    data: Value,
) -> Result<LedgerEntry, String> {
    let _guard = LEDGER_APPEND_LOCK
        .lock()
        .map_err(|_| "session ledger append lock poisoned")?;
    append_ledger_entry_unlocked(dir, ts, kind, data)
}

fn append_ledger_entry_unlocked(
    dir: &Path,
    ts: String,
    kind: &str,
    data: Value,
) -> Result<LedgerEntry, String> {
    let payload = SessionPayloadV2::from_legacy(kind, data)?;
    ledger_v2::store::append(dir, ts, payload).map(|event| LedgerEntry::from_event(&event))
}

pub(crate) fn append_checkpoint_entry(
    dir: &Path,
    ts: String,
    label: Option<String>,
    messages: &[Value],
) -> Result<LedgerEntry, String> {
    let _guard = LEDGER_APPEND_LOCK
        .lock()
        .map_err(|_| "session ledger append lock poisoned")?;
    let payload = SessionPayloadV2::checkpoint(CheckpointPayloadV2 {
        label,
        messages: messages.to_vec(),
    })?;
    ledger_v2::store::append(dir, ts, payload).map(|event| LedgerEntry::from_event(&event))
}

pub(crate) fn append_rewind_entry(
    dir: &Path,
    ts: String,
    checkpoint_id: &str,
) -> Result<(LedgerEntry, Vec<Value>), String> {
    let _guard = LEDGER_APPEND_LOCK
        .lock()
        .map_err(|_| "session ledger append lock poisoned")?;
    let ledger = parse_transcript(dir)?;
    if unresolved_pending_claim(&ledger.active_entries).is_some() {
        return Err("cannot rewind while a pending action claim is unresolved".into());
    }
    let global = ledger
        .events
        .iter()
        .find(|event| event.event_id == checkpoint_id);
    if !ledger
        .active_entries
        .iter()
        .any(|entry| entry.id == checkpoint_id)
    {
        return match global {
            Some(event) if !matches!(event.payload, SessionPayloadV2::Checkpoint(_)) => {
                Err(format!("event {checkpoint_id} is not a checkpoint"))
            }
            Some(_) => Err(format!(
                "checkpoint {checkpoint_id} is not an ancestor of active leaf {:?}",
                ledger.active_leaf
            )),
            None => Err(format!("checkpoint not found: {checkpoint_id}")),
        };
    }
    let checkpoint = global.ok_or_else(|| format!("checkpoint not found: {checkpoint_id}"))?;
    if !matches!(checkpoint.payload, SessionPayloadV2::Checkpoint(_)) {
        return Err(format!("event {checkpoint_id} is not a checkpoint"));
    }
    let checkpoint_data = checkpoint
        .payload
        .checkpoint_data()
        .map_err(|error| format!("checkpoint {checkpoint_id} has invalid payload: {error}"))?;
    let from_leaf_id = ledger
        .active_leaf
        .clone()
        .ok_or("cannot rewind an empty session ledger")?;
    let payload = SessionPayloadV2::rewind(RewindPayloadV2 {
        checkpoint_id: checkpoint_id.to_owned(),
        from_leaf_id,
    })?;
    let event =
        ledger_v2::store::append_with_parent(dir, ts, payload, Some(checkpoint_id.to_owned()))?;
    Ok((LedgerEntry::from_event(&event), checkpoint_data.messages))
}

pub(crate) fn session_active_leaf(dir: &Path) -> Result<Option<String>, String> {
    ledger_v2::store::reconcile_active_leaf(dir)
}

fn export_event(event: &SessionEventV2) -> Value {
    let mut value = serde_json::to_value(event).unwrap_or(Value::Null);
    if let Some(object) = value.as_object_mut() {
        object.insert("version".into(), json!(event.schema_version));
        object.insert("id".into(), json!(event.event_id));
        object.insert("parentId".into(), json!(event.parent_id));
        object.insert("ts".into(), json!(event.timestamp));
        object.insert("type".into(), json!(event.payload.kind()));
        object.insert("data".into(), event.payload.data().clone());
    }
    value
}
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
    if operation
        .deadline()
        .is_some_and(|deadline| std::time::Instant::now() >= deadline)
    {
        return Err("pending action deadline exceeded".into());
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

#[derive(Debug)]
struct SessionLedger {
    entries: Vec<LedgerEntry>,
    events: Vec<SessionEventV2>,
    active_entries: Vec<LedgerEntry>,
    active_leaf: Option<String>,
    recovered_truncated_tail: bool,
}

use crate::{agent, read_json, session_root, Args};

pub(crate) fn list_sessions(limit: Option<usize>) -> String {
    let mut rows = vec![];
    if let Ok(entries) = fs::read_dir(session_root()) {
        for entry in entries.flatten().take(limit.unwrap_or(usize::MAX)) {
            rows.push(entry.file_name().to_string_lossy().to_string());
        }
    }
    if rows.is_empty() {
        "No sessions found.\n".into()
    } else {
        rows.join("\n") + "\n"
    }
}

pub(crate) fn search_sessions_command(args: &Args) -> Result<String, String> {
    let query = args
        .positionals
        .first()
        .ok_or("search-sessions requires a query")?
        .trim()
        .to_ascii_lowercase();
    if query.is_empty() {
        return Err("search-sessions requires a non-empty query".into());
    }
    // Optional positional limit; absent means scan every session (the prior
    // default/clamp were unconsented numeric literals and are dropped).
    let limit = args
        .positionals
        .split_first()
        .and_then(|(_, rest)| rest.first())
        .and_then(|value| value.parse::<usize>().ok());
    let mut rows = Vec::new();
    if let Ok(entries) = fs::read_dir(session_root()) {
        let mut entries = entries
            .flatten()
            .map(|entry| entry.path())
            .collect::<Vec<_>>();
        entries.sort_by(|a, b| b.file_name().cmp(&a.file_name()));
        for dir in entries.into_iter().take(limit.unwrap_or(usize::MAX)) {
            let session = read_session_value(&dir.display().to_string())
                .map_err(|error| format!("cannot search session {}: {}", dir.display(), error))?;
            let id = session.get("id").and_then(Value::as_str).unwrap_or("");
            let events = session
                .get("events")
                .and_then(Value::as_array)
                .cloned()
                .unwrap_or_default();
            for event in events {
                let text = serde_json::to_string(event.get("data").unwrap_or(&Value::Null))
                    .unwrap_or_default();
                let lower = text.to_ascii_lowercase();
                if !lower.contains(&query) {
                    continue;
                }
                // Whitespace-collapsed full event text (the prior fixed-width
                // char window was an unconsented numeric literal).
                let snippet = text.split_whitespace().collect::<Vec<_>>().join(" ");
                rows.push(format!(
                    "{}\t{}\t{}\t{}",
                    id,
                    event.get("ts").and_then(Value::as_str).unwrap_or(""),
                    event.get("type").and_then(Value::as_str).unwrap_or(""),
                    snippet
                ));
                break;
            }
        }
    }
    Ok(if rows.is_empty() {
        String::new()
    } else {
        rows.join("\n") + "\n"
    })
}

pub(crate) fn session_dir_for(id_or_path: &str) -> PathBuf {
    if id_or_path.contains('/') {
        PathBuf::from(id_or_path)
    } else {
        session_root().join(id_or_path)
    }
}

/// `jeden resume <id-or-path> ["<task>"]`: load a recorded session's turns into
/// a fresh conversation and, when a task is given, continue it with a real turn
/// (a genuine in-process resume, not just inspection). Accepts `--allow-write`,
/// `--allow-command`, and `--yolo`/`--auto-approve` among the trailing args
/// because the resume parser deliberately skips normal flag handling.
pub(crate) fn resume_command(args: &Args) -> Result<String, String> {
    let (id, rest) = args
        .positionals
        .split_first()
        .ok_or("Usage: jeden resume <session-id-or-path> [\"<task>\"]")?;
    let dir = session_dir_for(id);
    if !dir.exists() {
        return Err(format!("session not found: {}", dir.display()));
    }
    let turns = session_conversation_turns(&dir)?;
    let count = turns.len();
    let mut allow_write = false;
    let mut allow_command = false;
    let mut yolo = false;
    let mut task_parts = Vec::new();
    for part in rest {
        match part.as_str() {
            "--allow-write" => allow_write = true,
            "--allow-command" => allow_command = true,
            "--yolo" | "--auto-approve" => {
                yolo = true;
                allow_write = true;
                allow_command = true;
            }
            other => task_parts.push(other.to_string()),
        }
    }
    let task = task_parts.join(" ").trim().to_string();
    let mut conversation = agent::Conversation::new(&args.cwd)?;
    conversation.load_history(&args.cwd, turns)?;
    if task.is_empty() {
        return Ok(format!(
            "Loaded {} prior turn(s) from {} into a new session. Continue with: jeden resume {} \"<task>\"\n",
            count, dir.display(), id
        ));
    }
    let mut run_args = args.clone();
    run_args.allow_write = allow_write;
    run_args.allow_command = allow_command;
    run_args.yolo = yolo;
    let mut hooks = agent::RunHooks::inert();
    let text = conversation.run_turn(&run_args, &task, &[], &mut hooks)?;
    let _ = agent::update_last_session_path(&args.cwd, &conversation.session_path());
    Ok(format!(
        "[resumed {} prior turn(s) from {}]\n{}\n",
        count,
        dir.display(),
        text
    ))
}

/// `jeden recall_conversation <id-or-path>`: render a recorded session's full
/// transcript as markdown (recall/inspection).
pub(crate) fn recall_conversation_command(args: &Args) -> Result<String, String> {
    let id = args
        .positionals
        .first()
        .ok_or("Usage: jeden recall_conversation <session-id-or-path>")?;
    let value = read_session_value(id)?;
    render_session_export(&value, "markdown")
}

/// Text-only transcript of a recorded session — user prompts and final answers
/// only, with tool calls/results and images stripped. Mirrors the external
/// `recall_conversation.sh` extraction, exposed so the agent `recall_conversation`
/// tool can reload a session's readable history into context.
pub(crate) fn recall_conversation_text(id_or_path: &str) -> Result<String, String> {
    let dir = session_dir_for(id_or_path);
    if !dir.exists() {
        return Err(format!("session not found: {}", dir.display()));
    }
    let turns = session_conversation_turns(&dir)?;
    if turns.is_empty() {
        return Ok(String::new());
    }
    let body = turns
        .iter()
        .map(|turn| {
            let role = turn
                .get("role")
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_ascii_uppercase();
            let content = turn.get("content").and_then(Value::as_str).unwrap_or("");
            format!("[{}]\n{}", role, content)
        })
        .collect::<Vec<_>>()
        .join("\n\n");
    Ok(body)
}

fn parse_transcript(dir: &Path) -> Result<SessionLedger, String> {
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

fn replay_entries(entries: Vec<LedgerEntry>) -> Result<Vec<Value>, String> {
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
            "final" => {
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

pub(crate) fn read_session_value(id_or_path: &str) -> Result<Value, String> {
    let dir = session_dir_for(id_or_path);
    if !dir.exists() {
        return Err(format!("session not found: {}", dir.display()));
    }
    let state: Value = read_json(&dir.join("state.json"));
    let ledger = parse_transcript(&dir)?;
    let id = dir
        .file_name()
        .map(|v| v.to_string_lossy().to_string())
        .unwrap_or_else(|| id_or_path.to_string());
    let events = ledger.events.iter().map(export_event).collect::<Vec<_>>();
    Ok(json!({
        "id": id,
        "path": dir,
        "state": state,
        "ledgerVersion": SESSION_LEDGER_VERSION,
        "activeLeaf": ledger.active_leaf,
        "recoveredTruncatedTail": ledger.recovered_truncated_tail,
        "events": events,
    }))
}

fn html_escape(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

pub(crate) fn render_session_export(session: &Value, format: &str) -> Result<String, String> {
    if format == "json" {
        return Ok(serde_json::to_string_pretty(session).map_err(|e| e.to_string())? + "\n");
    }
    let id = session
        .get("id")
        .and_then(Value::as_str)
        .unwrap_or("session");
    let path = session.get("path").and_then(Value::as_str).unwrap_or("");
    let events = session
        .get("events")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    if format == "markdown" || format == "md" {
        let mut out = format!("# Jeden session {}\n\n{}\n\n", id, path);
        for event in events {
            let label = format!(
                "{} {}",
                event.get("ts").and_then(Value::as_str).unwrap_or(""),
                event.get("type").and_then(Value::as_str).unwrap_or("")
            )
            .trim()
            .to_string();
            let data = serde_json::to_string_pretty(event.get("data").unwrap_or(&Value::Null))
                .unwrap_or_else(|_| "{}".into());
            out.push_str(&format!("## {}\n\n```json\n{}\n```\n\n", label, data));
        }
        return Ok(out);
    }
    if format == "html" {
        let mut sections = String::new();
        for event in events {
            let label = html_escape(
                format!(
                    "{} {}",
                    event.get("ts").and_then(Value::as_str).unwrap_or(""),
                    event.get("type").and_then(Value::as_str).unwrap_or("")
                )
                .trim(),
            );
            let body = html_escape(
                &serde_json::to_string_pretty(event.get("data").unwrap_or(&Value::Null))
                    .unwrap_or_else(|_| "{}".into()),
            );
            sections.push_str(&format!(
                "<section class=\"event\"><h2>{}</h2><pre>{}</pre></section>\n",
                label, body
            ));
        }
        return Ok(format!("<!doctype html>\n<html><head><meta charset=\"utf-8\"><title>Jeden session {}</title><style>body{{font-family:ui-sans-serif,system-ui,sans-serif;margin:2rem;background:#fafafa;color:#111}}.event{{border:1px solid #ddd;border-radius:8px;background:white;margin:1rem 0;padding:1rem}}pre{{white-space:pre-wrap;overflow-wrap:anywhere}}</style></head><body><h1>Jeden session {}</h1><p>{}</p>{}</body></html>\n", html_escape(id), html_escape(id), html_escape(path), sections));
    }
    Err(format!("unsupported session export format: {}", format))
}

pub(crate) fn export_session_command(args: &Args) -> Result<String, String> {
    let (id, rest) = args
        .positionals
        .split_first()
        .ok_or("export requires a session id or path")?;
    let mut format = "json".to_string();
    let mut output = None;
    for arg in rest {
        if arg == "--html" {
            format = "html".into();
        } else if arg == "--markdown" {
            format = "markdown".into();
        } else {
            output = Some(arg.clone());
        }
    }
    let payload = render_session_export(&read_session_value(id)?, &format)?;
    if let Some(path) = output {
        fs::write(&path, &payload).map_err(|e| e.to_string())?;
        Ok(format!("{}\n", path))
    } else {
        Ok(payload)
    }
}

pub(crate) fn list_artifacts_command(id_or_path: &str) -> Result<String, String> {
    let dir = session_dir_for(id_or_path).join("artifacts");
    let mut rows = vec![];
    if let Ok(entries) = fs::read_dir(dir) {
        for entry in entries.flatten() {
            if let Ok(meta) = entry.metadata() {
                if meta.is_file() {
                    rows.push(format!(
                        "{}\t{}",
                        entry.file_name().to_string_lossy(),
                        meta.len()
                    ));
                }
            }
        }
    }
    rows.sort();
    Ok(if rows.is_empty() {
        String::new()
    } else {
        rows.join("\n") + "\n"
    })
}

pub(crate) fn artifact_command(args: &Args) -> Result<String, String> {
    let mut it = args.positionals.iter();
    let id = it.next().ok_or("artifact requires a session id or path")?;
    let name = it.next().ok_or("artifact requires an artifact name")?;
    let output = it.next();
    let root = session_dir_for(id).join("artifacts");
    let file = root.join(name);
    let canonical_root = fs::canonicalize(&root).map_err(|e| e.to_string())?;
    let canonical_file = fs::canonicalize(&file).map_err(|e| e.to_string())?;
    if !canonical_file.starts_with(&canonical_root) {
        return Err(format!("artifact path escapes session: {}", name));
    }
    let content = fs::read_to_string(&canonical_file).map_err(|e| e.to_string())?;
    if let Some(output) = output {
        fs::write(output, &content).map_err(|e| e.to_string())?;
        Ok(format!("{}\n", output))
    } else {
        Ok(if content.ends_with('\n') {
            content
        } else {
            content + "\n"
        })
    }
}
