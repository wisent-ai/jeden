//! Session CLI subcommands: list, show, export, artifacts, resume, recall.

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Mutex;

// Relative to this file's directory, rust/cli/reports/sessions/. The file
// has moved twice (cli/ to cli/reports/, then into this directory) and the
// path kept pointing one level up, at a rust/cli/session/ that never existed.
#[path = "../../../session/mod.rs"]
pub(crate) mod ledger_v2;
use ledger_v2::SessionEventV2;

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

mod export;
mod ledger;
mod pending;
mod replay;

pub(crate) use export::{
    artifact_command, export_session_command, list_artifacts_command, render_session_export,
};
pub(crate) use ledger::{
    append_checkpoint_entry, append_ledger_entry, append_rewind_entry, session_active_leaf,
};
pub(crate) use pending::{
    claim_pending_action, complete_pending_action, create_pending_action, discard_pending_action,
    PendingActionClaim, PendingActionCreate,
};
pub(crate) use replay::{list_checkpoint_entries, session_conversation_turns};
use replay::{parse_transcript, replay_entries};

#[derive(Debug)]
struct SessionLedger {
    entries: Vec<LedgerEntry>,
    events: Vec<SessionEventV2>,
    active_entries: Vec<LedgerEntry>,
    active_leaf: Option<String>,
    recovered_truncated_tail: bool,
}

use crate::{agent, read_json, session_root, Args};
use ledger::export_event;

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
    let mut run_args = args.clone();
    if !args.cwd_explicit {
        run_args.cwd = crate::completion::cli::workspace(&dir)?;
    }
    let mut conversation = agent::Conversation::new(&run_args.cwd)?;
    conversation.load_history(&run_args.cwd, turns, &dir)?;
    agent::update_last_session_path(&run_args.cwd, &conversation.session_path())?;
    run_args.allow_write = allow_write;
    run_args.allow_command = allow_command;
    run_args.yolo = yolo;
    let mut hooks = agent::RunHooks::inert();
    let text = if task.is_empty() {
        conversation.continue_work(&run_args, &mut hooks)?
    } else {
        conversation.run_turn(&run_args, &task, &[], &mut hooks)?
    };
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
