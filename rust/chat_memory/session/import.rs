//! Bringing an earlier session into a chat: every message of the session's
//! conversation, as the session replays it, is appended to the chat's log as
//! a `note` (what the design calls memories from before the chat), naming
//! the session it came from and what the message was. The view then
//! summarizes it like any other message, so an agent that starts a chat
//! does not start from nothing. A session is imported into a chat once; the
//! chat keeps which sessions it imported in `imports.json`.

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::path::{Path, PathBuf};

use super::super::chat::{self, Chat};
use super::super::log::now;
use super::super::{Entry, Kind, Log};
use super::entries::{delegations, entries_of, own};
use super::{busy, own_chat};

/// One session a chat imported.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
struct Imported {
    /// The session directory it came from.
    session: PathBuf,
    /// The log index of its first message.
    first: u64,
    /// How many messages it brought.
    messages: usize,
    /// When it was imported.
    date: String,
}

fn imports_path(dir: &Path) -> PathBuf {
    dir.join("imports.json")
}

fn read_imports(dir: &Path) -> Result<Vec<Imported>, String> {
    let path = imports_path(dir);
    match std::fs::read_to_string(&path) {
        Ok(text) => serde_json::from_str(&text)
            .map_err(|error| format!("{} is not a list of imports ({error})", path.display())),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(Vec::new()),
        Err(error) => Err(format!("{} cannot be read: {error}", path.display())),
    }
}

/// Import the session `source` (an id or directory) into the chat named
/// `name`, creating the chat when it has never been written. Refused when
/// another process writes the chat, when the chat imported that session
/// already, or when the session cannot be replayed.
pub fn import(name: &str, source: &str, cwd: &Path) -> Result<Value, String> {
    let chat = Chat::named(name, "the import")?;
    let session = crate::cli::sessions::recorded_session_dir(source)?;
    let session = session.canonicalize().map_err(|error| {
        format!(
            "the session {} cannot be resolved: {error}",
            session.display()
        )
    })?;
    let _owner = own_chat(&chat)?;
    chat::record(&chat, cwd)?;
    let _guard = busy();
    let mut imports = read_imports(&chat.dir)?;
    if let Some(earlier) = imports.iter().find(|earlier| earlier.session == session) {
        return Err(format!(
            "the chat `{}` imported the session {} on {} already, as messages {} to {}; it is not imported twice",
            chat.name,
            session.display(),
            earlier.date,
            earlier.first,
            earlier.first + earlier.messages as u64
        ));
    }
    let turns = crate::cli::sessions::session_conversation_turns(&session)?;
    let id = session
        .file_name()
        .map(|id| id.to_string_lossy().into_owned())
        .ok_or_else(|| format!("the session {} has no name", session.display()))?;
    let log = Log::at(&chat.dir);
    let first = log.read()?.len() as u64;
    let date = now()?;
    let mut notes: Vec<Entry> = Vec::new();
    let delegated = delegations(own(&turns));
    for message in own(&turns) {
        let next = first + notes.len() as u64;
        for entry in entries_of(message, next, &date, &delegated)? {
            let said = format!("[imported from session {id}] {}", entry.labelled()?);
            notes.push(Entry::new(
                first + notes.len() as u64,
                Kind::Note,
                said,
                date.clone(),
            ));
        }
    }
    if notes.is_empty() {
        return Err(format!(
            "the session {} holds no conversation message to import",
            session.display()
        ));
    }
    log.append(&notes)?;
    imports.push(Imported {
        session: session.clone(),
        first,
        messages: notes.len(),
        date,
    });
    let path = imports_path(&chat.dir);
    let text = serde_json::to_string_pretty(&imports)
        .map_err(|error| format!("the chat's imports cannot be encoded: {error}"))?;
    crate::tool_runtime::runtime_ops::durable::replace(&path, (text + "\n").as_bytes())?;
    Ok(json!({
        "chat": chat.name,
        "directory": chat.dir,
        "session": session,
        "first": first,
        "messages": notes.len(),
    }))
}
