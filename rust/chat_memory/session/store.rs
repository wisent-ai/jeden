//! What a chat memory keeps beside its log for the conversation that writes
//! it: which conversation messages the log already holds, the lock that
//! makes one process its writer, and the note a rewind leaves.

use std::fs::File;
use std::path::{Path, PathBuf};

use super::super::log::now;
use super::super::{Entry, Kind, Log};

/// Where the first log entry of each of the conversation's own messages
/// is kept, so a resumed session knows which of its messages the log holds.
fn taken_path(dir: &Path) -> PathBuf {
    dir.join("taken.json")
}

pub(super) fn load_taken(dir: &Path) -> Result<Vec<u64>, String> {
    let path = taken_path(dir);
    match std::fs::read_to_string(&path) {
        Ok(text) => serde_json::from_str(&text)
            .map_err(|error| format!("{} is not a list of log indices ({error})", path.display())),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(Vec::new()),
        Err(error) => Err(format!("{} cannot be read: {error}", path.display())),
    }
}

pub(super) fn save_taken(dir: &Path, taken: &[u64]) -> Result<(), String> {
    let path = taken_path(dir);
    let staged = dir.join("taken.json.staged");
    let text = serde_json::to_string(taken)
        .map_err(|error| format!("the chat memory's taken list cannot be encoded: {error}"))?;
    std::fs::write(&staged, text)
        .and_then(|()| std::fs::rename(&staged, &path))
        .map_err(|error| format!("{} cannot be written: {error}", path.display()))
}

/// Hold the chat memory for this process: one process writes a chat. A
/// chat another process holds is refused naming the lock.
pub(super) fn own_chat(dir: &Path) -> Result<File, String> {
    use std::os::unix::io::AsRawFd;
    std::fs::create_dir_all(dir).map_err(|error| {
        format!(
            "the chat memory directory {} cannot be created: {error}",
            dir.display()
        )
    })?;
    let path = dir.join("owner.lock");
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
            "another process writes this chat memory (it holds {}); one process writes a chat",
            path.display()
        ))
    } else {
        Err(format!("{} cannot be locked: {error}", path.display()))
    }
}

/// A chat memory kept by a Jeden before this layout is refused, never
/// mixed with this one.
pub(super) fn refuse_earlier_layout(dir: &Path) -> Result<(), String> {
    let earlier = dir.join("log.jsonl");
    if earlier.exists() {
        return Err(format!(
            "{} is a chat log in the layout of an earlier Jeden; move {} aside to start this chat's memory in main/, tree/ and view.json",
            earlier.display(),
            dir.display()
        ));
    }
    Ok(())
}

/// Record that the conversation was taken back to `kept` of its own
/// messages: the log keeps every message it said, and a note after them
/// names the ones taken back. Nothing taken back is nothing to note.
pub(super) fn note_rewind(dir: &Path, taken: &mut Vec<u64>, kept: usize) -> Result<(), String> {
    let Some(&from) = taken.get(kept) else {
        return Ok(());
    };
    let log = Log::at(dir);
    let next = log.read()?.len() as u64;
    let text = format!(
        "rewind: the conversation was taken back to a checkpoint; the messages from {from} before this note were taken back, and the chat goes on from the messages before {from}"
    );
    log.append(&[Entry::new(next, Kind::Note, text, now()?)])?;
    taken.truncate(kept);
    save_taken(dir, taken)
}
