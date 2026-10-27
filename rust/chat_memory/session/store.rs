//! What a session keeps about the chat it writes to, and what the chat
//! keeps for its one writer.
//!
//! A session records, in `<session>/chat-memory.json`, which chat it writes
//! to and the first log index of each of its own conversation messages the
//! log holds (`taken`), so a resumed session knows which of its messages the
//! log already has. A session that has written no record yet (a child made
//! by `/reset`, a resume, `/fork` or `/branch`) starts from its parent's,
//! read through the lineage in its `state.json`, when the parent wrote to the
//! same chat: the history it carries is already in the log.

use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::fs::File;
use std::path::{Path, PathBuf};

use super::super::chat::Chat;
use super::super::log::now;
use super::super::{Entry, Kind, Log};

/// What a session records about its chat.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
struct SessionRecord {
    /// The chat directory the session writes to.
    chat: PathBuf,
    /// The first log index of each of the session's own messages the log holds.
    taken: Vec<u64>,
}

/// Where a session records its chat.
pub fn record_path(session_dir: &Path) -> PathBuf {
    session_dir.join("chat-memory.json")
}

fn read_record(session_dir: &Path) -> Result<Option<SessionRecord>, String> {
    let path = record_path(session_dir);
    match std::fs::read_to_string(&path) {
        Ok(text) => serde_json::from_str(&text).map(Some).map_err(|error| {
            format!(
                "{} is not a session's chat record ({error})",
                path.display()
            )
        }),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(format!("{} cannot be read: {error}", path.display())),
    }
}

/// The chat directory the session at `session_dir` wrote to, none when it
/// wrote to none.
pub fn session_chat(session_dir: &Path) -> Result<Option<PathBuf>, String> {
    Ok(read_record(session_dir)?.map(|record| record.chat))
}

/// The session the one at `session_dir` was made from, as its `state.json`
/// lineage names it; none for a session made from none.
fn parent_of(session_dir: &Path) -> Result<Option<PathBuf>, String> {
    let path = session_dir.join("state.json");
    let text = match std::fs::read_to_string(&path) {
        Ok(text) => text,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(format!("{} cannot be read: {error}", path.display())),
    };
    let state: Value = serde_json::from_str(&text)
        .map_err(|error| format!("{} is not a session state ({error})", path.display()))?;
    Ok(state
        .get("lineage")
        .and_then(|lineage| lineage.get("parentSession"))
        .and_then(Value::as_str)
        .map(PathBuf::from))
}

/// The record the session goes on from: its own, else the one of the session
/// it was made from.
fn lineage_record(session_dir: &Path) -> Result<Option<SessionRecord>, String> {
    match read_record(session_dir)? {
        Some(record) => Ok(Some(record)),
        None => match parent_of(session_dir)? {
            Some(parent) => read_record(&parent),
            None => Ok(None),
        },
    }
}

/// The chat the session at `session_dir` already writes to, or the one the
/// session it was made from writes to; none for a session of no chat yet.
pub(super) fn lineage_chat(session_dir: &Path) -> Result<Option<PathBuf>, String> {
    Ok(lineage_record(session_dir)?.map(|record| record.chat))
}

/// Which of the session's own messages `chat` already holds: the session's
/// own record, else its parent's, when either names this chat. A session
/// whose records name another chat or none has none of its messages there.
pub(super) fn load_taken(session_dir: &Path, chat: &Path) -> Result<Vec<u64>, String> {
    match lineage_record(session_dir)? {
        Some(record) if record.chat == chat => Ok(record.taken),
        Some(_) | None => Ok(Vec::new()),
    }
}

pub(super) fn save_taken(session_dir: &Path, chat: &Path, taken: &[u64]) -> Result<(), String> {
    let path = record_path(session_dir);
    let record = SessionRecord {
        chat: chat.to_path_buf(),
        taken: taken.to_vec(),
    };
    let text = serde_json::to_string(&record)
        .map_err(|error| format!("the session's chat record cannot be encoded: {error}"))?;
    crate::tool_runtime::runtime_ops::durable::replace(&path, text.as_bytes())
}

/// Hold the chat for this process: one process writes a chat. A chat
/// another process holds is refused naming the chat, the lock and how to
/// give this agent a chat of its own.
pub(super) fn own_chat(chat: &Chat) -> Result<File, String> {
    use std::os::unix::io::AsRawFd;
    std::fs::create_dir_all(&chat.dir).map_err(|error| {
        format!(
            "the chat directory {} cannot be created: {error}",
            chat.dir.display()
        )
    })?;
    let path = chat.dir.join("owner.lock");
    let file = std::fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .open(&path)
        .map_err(|error| format!("{} cannot be opened: {error}", path.display()))?;
    // SAFETY: flock on a descriptor this process owns.
    let locked = unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) };
    if locked == libc::EXIT_SUCCESS {
        return Ok(file);
    }
    let error = std::io::Error::last_os_error();
    if error.raw_os_error() == Some(libc::EWOULDBLOCK) {
        Err(format!(
            "the chat `{}` is written by another process now (it holds {}); one process writes a chat, so give this agent its own with {}=<name> or chatMemory.chat",
            chat.name,
            path.display(),
            super::super::chat::CHAT_ENV
        ))
    } else {
        Err(format!("{} cannot be locked: {error}", path.display()))
    }
}

/// Record that the conversation was taken back to `kept` of its own
/// messages: the log keeps every message it said, and a note after them
/// names the ones taken back. Nothing taken back is nothing to note.
pub(super) fn note_rewind(
    chat: &Path,
    session_dir: &Path,
    taken: &mut Vec<u64>,
    kept: usize,
) -> Result<(), String> {
    let Some(&from) = taken.get(kept) else {
        return Ok(());
    };
    let log = Log::at(chat);
    let next = log.read()?.len() as u64;
    let text = format!(
        "rewind: the conversation was taken back to a checkpoint; the messages from {from} before this note were taken back, and the chat goes on from the messages before {from}"
    );
    log.append(&[Entry::new(next, Kind::Note, text, now()?)])?;
    taken.truncate(kept);
    save_taken(session_dir, chat, taken)
}
