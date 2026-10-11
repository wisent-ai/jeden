//! The chat's own log: every message the conversation holds, user prompts,
//! replies, tool calls and the tool results as the model received them, word
//! for word, one JSON line each, appended and never rewritten.
//!
//! The session ledger records events (actions, routes, receipts); this log
//! records the messages themselves in the order the model saw them, which is
//! what the summary tree covers and what a zoom answers with.

use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};

/// One message of the chat, as the conversation held it.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct Entry {
    /// Its place in the chat, from the first message on.
    pub index: u64,
    /// When it entered the log, RFC 3339.
    pub at: String,
    pub role: String,
    /// The message's content exactly as the model saw it.
    pub content: Value,
}

impl Entry {
    /// The content as text: a string as it is, anything else as its JSON.
    pub fn text(&self) -> String {
        match &self.content {
            Value::String(text) => text.clone(),
            other => other.to_string(),
        }
    }

    /// How many bytes the entry takes in a view: its role and its text.
    pub fn bytes(&self) -> usize {
        self.role.len() + self.text().len()
    }
}

/// The append-only log file of one session.
pub struct Log {
    path: PathBuf,
}

impl Log {
    pub fn at(dir: &Path) -> Self {
        Self {
            path: dir.join("log.jsonl"),
        }
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Every entry, in order. No file is an empty chat; a line that is not an
    /// entry, or an entry out of order, is refused naming where it stands,
    /// never skipped.
    pub fn read(&self) -> Result<Vec<Entry>, String> {
        let text = match fs::read_to_string(&self.path) {
            Ok(text) => text,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
            Err(error) => {
                return Err(format!(
                    "the chat log {} cannot be read: {error}",
                    self.path.display()
                ))
            }
        };
        let mut entries: Vec<Entry> = Vec::new();
        for line in text.lines().filter(|line| !line.trim().is_empty()) {
            let entry: Entry = serde_json::from_str(line).map_err(|error| {
                format!(
                    "the chat log {} holds, after its {} message(s), a line that is not an entry ({error})",
                    self.path.display(),
                    entries.len()
                )
            })?;
            if entry.index != entries.len() as u64 {
                return Err(format!(
                    "the chat log {} holds message {} where message {} belongs",
                    self.path.display(),
                    entry.index,
                    entries.len()
                ));
            }
            entries.push(entry);
        }
        Ok(entries)
    }

    /// Append `entries` at the end, written whole and flushed to disk before
    /// the call answers.
    pub fn append(&self, entries: &[Entry]) -> Result<(), String> {
        if entries.is_empty() {
            return Ok(());
        }
        if let Some(parent) = self.path.parent() {
            fs::create_dir_all(parent).map_err(|error| {
                format!(
                    "the chat memory directory {} cannot be created: {error}",
                    parent.display()
                )
            })?;
        }
        let mut text = String::new();
        for entry in entries {
            let line = serde_json::to_string(entry)
                .map_err(|error| format!("message {} cannot be encoded: {error}", entry.index))?;
            text.push_str(&line);
            text.push('\n');
        }
        let mut file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&self.path)
            .map_err(|error| {
                format!(
                    "the chat log {} cannot be opened: {error}",
                    self.path.display()
                )
            })?;
        file.write_all(text.as_bytes())
            .and_then(|()| file.sync_all())
            .map_err(|error| {
                format!(
                    "the chat log {} cannot be appended to: {error}",
                    self.path.display()
                )
            })
    }
}
