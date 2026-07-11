//! Session CLI subcommands: list, show, export, artifacts, resume, recall.

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Mutex;

static LEDGER_APPEND_LOCK: Mutex<()> = Mutex::new(());
static NEXT_ENTRY_ID: AtomicU64 = AtomicU64::new(1);

pub(crate) const SESSION_LEDGER_VERSION: u32 = 1;

/// Versioned on-disk envelope. `kind` and `data` deliberately retain the
/// existing event vocabulary so export/search remain backwards compatible.
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
    fn export_value(&self) -> Value {
        json!({
            "version": self.version,
            "id": self.id,
            "parentId": self.parent_id,
            "ts": self.ts,
            "type": self.kind,
            "data": self.data,
        })
    }
}
pub(crate) fn append_ledger_entry(dir: &Path, ts: String, kind: &str, data: Value) -> Result<LedgerEntry, String> {
    let _guard = LEDGER_APPEND_LOCK.lock().map_err(|_| "session ledger append lock poisoned")?;
    let ledger = parse_transcript(dir)?;
    if ledger.recovered_truncated_tail {
        return Err(format!("cannot append {}: transcript has a recovered truncated tail; resume into a child session", dir.display()));
    }
    let id = format!("entry-{}-{:016x}", ts, NEXT_ENTRY_ID.fetch_add(1, Ordering::Relaxed));
    let entry = LedgerEntry {
        version: SESSION_LEDGER_VERSION,
        id: id.clone(),
        parent_id: ledger.active_leaf,
        ts,
        kind: kind.to_string(),
        data,
    };
    let mut file = OpenOptions::new().create(true).append(true)
        .open(dir.join("transcript.jsonl")).map_err(|e| e.to_string())?;
    writeln!(file, "{}", serde_json::to_string(&entry).map_err(|e| e.to_string())?)
        .map_err(|e| e.to_string())?;
    file.flush().map_err(|e| e.to_string())?;
    file.sync_data().map_err(|e| e.to_string())?;

    let state_path = dir.join("state.json");
    let text = fs::read_to_string(&state_path).map_err(|e| e.to_string())?;
    let mut state: Value = serde_json::from_str(&text)
        .map_err(|e| format!("invalid {}: {}", state_path.display(), e))?;
    let object = state.as_object_mut().ok_or_else(|| format!("invalid {}: expected object", state_path.display()))?;
    object.insert("activeLeaf".into(), json!(id));
    fs::write(&state_path, serde_json::to_string_pretty(&state).map_err(|e| e.to_string())? + "\n")
        .map_err(|e| e.to_string())?;
    Ok(entry)
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
    artifact_dir.parent().ok_or_else(|| "pending registry artifact directory has no session parent".into())
}

fn operation_ready(operation: &crate::tool_runtime::runtime_ops::OperationContext<'_>) -> Result<(), String> {
    if operation.cancellation().is_cancelled() { return Err("pending action cancelled".into()); }
    if operation.deadline().is_some_and(|deadline| std::time::Instant::now() >= deadline) {
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
    if create.payload.len() > MAX_PENDING_PAYLOAD_BYTES { return Err("pending action payload exceeds limit".into()); }
    let session_dir = pending_session_dir(artifact_dir)?;
    let now = now_epoch_seconds();
    let id = format!("pending-{}-{:016x}", now, NEXT_ENTRY_ID.fetch_add(1, Ordering::Relaxed));
    let dir = artifact_dir.join("pending-actions");
    fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let payload_path = dir.join(format!("{}.payload", id));
    fs::write(&payload_path, &create.payload).map_err(|e| e.to_string())?;
    append_ledger_entry(session_dir, now.to_string(), "pending_preview", json!({
        "pendingId": id,
        "kind": create.kind,
        "target": create.target,
        "expectedSha256": create.expected_sha256,
        "payloadPath": payload_path,
        "preview": create.preview,
        "createdAt": now,
        "expiresAt": now.saturating_add(create.ttl_seconds),
    }))?;
    Ok(id)
}

fn pending_terminal(entries: &[LedgerEntry], id: &str) -> bool {
    entries.iter().any(|entry| matches!(entry.kind.as_str(), "pending_claim" | "pending_apply" | "pending_discard" | "pending_expire")
        && entry.data.get("pendingId").and_then(Value::as_str) == Some(id))
}

pub(crate) fn claim_pending_action(
    artifact_dir: &Path,
    operation: &crate::tool_runtime::runtime_ops::OperationContext<'_>,
    id: &str,
) -> Result<PendingActionClaim, String> {
    operation_ready(operation)?;
    let session_dir = pending_session_dir(artifact_dir)?;
    let ledger = parse_transcript(session_dir)?;
    if pending_terminal(&ledger.entries, id) { return Err(format!("pending action is already resolved: {}", id)); }
    let entry = ledger.entries.iter().rev().find(|entry| entry.kind == "pending_preview"
        && entry.data.get("pendingId").and_then(Value::as_str) == Some(id))
        .ok_or_else(|| format!("pending action not found: {}", id))?;
    let expires_at = entry.data.get("expiresAt").and_then(Value::as_u64)
        .ok_or_else(|| format!("pending action {} has invalid expiry", id))?;
    let now = now_epoch_seconds();
    if now >= expires_at {
        append_ledger_entry(session_dir, now.to_string(), "pending_expire", json!({ "pendingId": id }))?;
        return Err(format!("pending action expired: {}", id));
    }
    let payload_path = artifact_dir.join("pending-actions").join(format!("{}.payload", id));
    let payload = fs::read(&payload_path).map_err(|e| format!("cannot read pending action {} payload: {}", id, e))?;
    operation_ready(operation)?;
    append_ledger_entry(session_dir, now.to_string(), "pending_claim", json!({ "pendingId": id }))?;
    let kind = entry.data.get("kind").and_then(Value::as_str)
        .ok_or_else(|| format!("pending action {} has invalid kind", id))?;
    let target = entry.data.get("target").and_then(Value::as_str)
        .ok_or_else(|| format!("pending action {} has invalid target", id))?;
    let expected_sha256 = entry.data.get("expectedSha256").and_then(Value::as_str)
        .ok_or_else(|| format!("pending action {} has invalid revision", id))?;
    Ok(PendingActionClaim {
        id: id.to_string(),
        kind: kind.to_string(),
        target: target.to_string(),
        expected_sha256: expected_sha256.to_string(),
        payload,
    })
}

pub(crate) fn complete_pending_action(artifact_dir: &Path, id: &str) -> Result<(), String> {
    let session_dir = pending_session_dir(artifact_dir)?;
    append_ledger_entry(session_dir, now_epoch_seconds().to_string(), "pending_apply", json!({ "pendingId": id }))?;
    remove_pending_payload(artifact_dir, id)
}

pub(crate) fn discard_pending_action(
    artifact_dir: &Path,
    operation: &crate::tool_runtime::runtime_ops::OperationContext<'_>,
    id: &str,
) -> Result<(), String> {
    operation_ready(operation)?;
    let session_dir = pending_session_dir(artifact_dir)?;
    let ledger = parse_transcript(session_dir)?;
    if pending_terminal(&ledger.entries, id) { return Err(format!("pending action is already resolved: {}", id)); }
    if !ledger.entries.iter().any(|entry| entry.kind == "pending_preview" && entry.data.get("pendingId").and_then(Value::as_str) == Some(id)) {
        return Err(format!("pending action not found: {}", id));
    }
    append_ledger_entry(session_dir, now_epoch_seconds().to_string(), "pending_discard", json!({ "pendingId": id }))?;
    remove_pending_payload(artifact_dir, id)
}

fn remove_pending_payload(artifact_dir: &Path, id: &str) -> Result<(), String> {
    match fs::remove_file(artifact_dir.join("pending-actions").join(format!("{}.payload", id))) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error.to_string()),
    }
}

fn now_epoch_seconds() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap_or_default().as_secs()
}

#[derive(Debug)]
struct SessionLedger {
    entries: Vec<LedgerEntry>,
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
    let text = conversation.run_turn(&run_args, &task, &mut hooks)?;
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
    let path = dir.join("transcript.jsonl");
    let bytes = fs::read(&path).map_err(|e| format!("cannot read {}: {}", path.display(), e))?;
    if bytes.is_empty() {
        return Ok(SessionLedger { entries: Vec::new(), active_leaf: None, recovered_truncated_tail: false });
    }
    let terminated = bytes.last().copied() == Some(b'\n');
    let chunks = bytes.split(|byte| *byte == b'\n').collect::<Vec<_>>();
    let logical_len = chunks.len().saturating_sub(usize::from(terminated));
    let mut entries = Vec::with_capacity(logical_len);
    let mut active_leaf: Option<String> = None;
    let mut recovered_truncated_tail = false;

    for (index, raw) in chunks.into_iter().take(logical_len).enumerate() {
        let line_number = index + 1;
        let is_recoverable_tail = !terminated && line_number == logical_len;
        let line = match std::str::from_utf8(raw) {
            Ok(line) => line,
            Err(error) if is_recoverable_tail => {
                let _ = error;
                recovered_truncated_tail = true;
                break;
            }
            Err(error) => return Err(format!("{}:{} is not UTF-8: {}", path.display(), line_number, error)),
        };
        let value: Value = match serde_json::from_str(line) {
            Ok(value) => value,
            Err(_) if is_recoverable_tail => {
                recovered_truncated_tail = true;
                break;
            }
            Err(error) => return Err(format!("{}:{} is malformed JSON: {}", path.display(), line_number, error)),
        };

        let entry = if value.get("version").is_some() {
            let entry: LedgerEntry = serde_json::from_value(value)
                .map_err(|e| format!("{}:{} is not a valid ledger entry: {}", path.display(), line_number, e))?;
            if entry.version != SESSION_LEDGER_VERSION {
                return Err(format!("{}:{} uses unsupported ledger version {}", path.display(), line_number, entry.version));
            }
            entry
        } else {
            // Legacy `{ts,type,data}` transcript events are migrated in memory.
            // The source is never rewritten, which also makes tail recovery safe.
            let ts = value.get("ts").and_then(Value::as_str)
                .ok_or_else(|| format!("{}:{} legacy event has no string ts", path.display(), line_number))?;
            let kind = value.get("type").and_then(Value::as_str)
                .ok_or_else(|| format!("{}:{} legacy event has no string type", path.display(), line_number))?;
            let data = value.get("data").cloned()
                .ok_or_else(|| format!("{}:{} legacy event has no data", path.display(), line_number))?;
            LedgerEntry {
                version: SESSION_LEDGER_VERSION,
                id: format!("legacy-{}", line_number),
                parent_id: active_leaf.clone(),
                ts: ts.to_string(),
                kind: kind.to_string(),
                data,
            }
        };
        if entry.parent_id != active_leaf {
            return Err(format!(
                "{}:{} breaks ledger lineage: parent {:?}, active leaf {:?}",
                path.display(), line_number, entry.parent_id, active_leaf
            ));
        }
        active_leaf = Some(entry.id.clone());
        entries.push(entry);
    }
    Ok(SessionLedger { entries, active_leaf, recovered_truncated_tail })
}

fn read_transcript_events(dir: &Path) -> Result<Vec<Value>, String> {
    Ok(parse_transcript(dir)?.entries.iter().map(LedgerEntry::export_value).collect())
}

/// Faithfully rebuild the model-visible message window. Snapshot entries are
/// durable cut points; older legacy events are replayed through adapters.
pub(crate) fn session_conversation_turns(dir: &Path) -> Result<Vec<Value>, String> {
    replay_entries(parse_transcript(dir)?.entries)
}

fn replay_entries(entries: Vec<LedgerEntry>) -> Result<Vec<Value>, String> {
    let mut messages = Vec::new();
    for entry in entries {
        let data = entry.data;
        match entry.kind.as_str() {
            "context_snapshot" => {
                messages = data.get("messages").and_then(Value::as_array).cloned()
                    .ok_or_else(|| format!("ledger entry {} has invalid context snapshot", entry.id))?;
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
                    if let Some(last) = messages.last_mut().filter(|message| message.get("role").and_then(Value::as_str) == Some("assistant")) {
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
                messages = vec![json!({ "role": "system", "content": format!("Prior conversation summary (compacted from {} messages):\n{}", before, summary), "_jedenNeedsBaseSystem": true })];
            }
            "auto_continue" => {
                if let Some(prompt) = data.get("prompt").and_then(Value::as_str) {
                    messages.push(json!({ "role": "user", "content": prompt }));
                }
            }
            _ => {}
        }
    }
    Ok(messages)
}

pub(crate) fn session_messages_at(dir: &Path, entry_id: &str) -> Result<Vec<Value>, String> {
    let mut entries = parse_transcript(dir)?.entries;
    let index = entries.iter().position(|entry| entry.id == entry_id)
        .ok_or_else(|| format!("checkpoint entry not found: {}", entry_id))?;
    entries.truncate(index + 1);
    replay_entries(entries)
}

pub(crate) fn read_session_value(id_or_path: &str) -> Result<Value, String> {
    let dir = session_dir_for(id_or_path);
    if !dir.exists() {
        return Err(format!("session not found: {}", dir.display()));
    }
    let state: Value = read_json(&dir.join("state.json"));
    let ledger = parse_transcript(&dir)?;
    let id = dir.file_name().map(|v| v.to_string_lossy().to_string())
        .unwrap_or_else(|| id_or_path.to_string());
    let events = ledger.entries.iter().map(LedgerEntry::export_value).collect::<Vec<_>>();
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
