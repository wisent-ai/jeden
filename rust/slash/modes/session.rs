use std::fs;
use std::path::{Path, PathBuf};

use crate::slash::common::split_head;
use crate::slash::state::ModeState;
use crate::slash::SlashContext;

/// List every session directory under `session_root`, one per line. The prior
/// "most recent N" display cap was an unconsented numeric literal and has been
/// removed; all sessions are listed.
fn list_sessions(session_root: &Path) -> String {
    let mut rows = Vec::new();
    if let Ok(entries) = fs::read_dir(session_root) {
        for entry in entries.flatten() { rows.push(entry.file_name().to_string_lossy().to_string()); }
    }
    if rows.is_empty() { "No sessions found.".into() } else { rows.join("\n") }
}

fn session_path(session_root: &Path, id_or_path: &str) -> PathBuf {
    if id_or_path.contains('/') { PathBuf::from(id_or_path) } else { session_root.join(id_or_path) }
}

pub(crate) fn handle_session(args: &str, context: &SlashContext<'_>) -> Result<String, String> {
    let (verb, _) = split_head(args);
    if verb.is_empty() || verb == "info" {
        return Ok(format!("Session: rust one-shot slash invocation\nWorkspace: {}\nSession root: {}\nRecorder: not active in this non-interactive Rust command", context.cwd.display(), context.session_root.display()));
    }
    if verb == "delete" { return Err("Refusing to delete the active session from inside itself. Exit Jeden, then remove the session directory explicitly if you still want this destructive action.".into()); }
    Err("Usage: /session [info|delete]".into())
}

pub(crate) fn handle_lifecycle(command: &str, args: &str, state: &mut ModeState, context: &SlashContext<'_>) -> Option<Result<String, String>> {
    match command {
        "/new" | "/fresh" => Some(Ok("Started a fresh logical turn context. Provider stream state is reset for the next prompt in this Jeden process.".into())),
        "/drop" => Some(Err("Refusing to delete the active session from inside itself. Use /new for a fresh context or exit and remove the session directory explicitly.".into())),
        "/shake" => {
            state.shake = if args.trim().is_empty() { "elide".into() } else { args.trim().into() };
            Some(Ok(format!("Shake mode applied locally: {}. Subsequent prompts will instruct the model to avoid relying on heavy prior artifacts unless re-read.", state.shake)))
        },
        "/resume" => {
            let (id, _) = split_head(args);
            if id.is_empty() { Some(Ok(list_sessions(context.session_root))) }
            else {
                let path = session_path(context.session_root, id);
                if path.exists() { Some(Ok(format!("Session {} exists at {}. Full in-place interactive resume is available through CLI: jeden resume {} \"<task>\"", path.file_name().map(|v| v.to_string_lossy()).unwrap_or_default(), path.display(), path.display()))) }
                else { Some(Err(format!("session not found: {}", path.display()))) }
            }
        },
        "/rename" => Some(Ok(format!("Session title set to: {}", if args.trim().is_empty() { "rust one-shot slash invocation" } else { args.trim() }))),
        "/move" => Some(Err("/move requires an active interactive session recorder; Rust one-shot slash commands cannot move a live recorder in this pass.".into())),
        _ => None,
    }
}
