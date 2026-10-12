//! The chat's own log: every message of the chat word for word, one JSON
//! line `{i, kind, text, size, date}` each, in `main/YYYY-MM-DD.jsonl`
//! files split by day only to stay small. Lines are appended and flushed,
//! never edited or deleted.

use serde::{Deserialize, Serialize};
use std::ffi::CStr;
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};

/// What a message is; the log writes it in lower case.
#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum Kind {
    /// The user's words.
    User,
    /// Jeden's replies.
    Jeden,
    /// Jeden's tool calls: the tool's name and its JSON input.
    Tool,
    /// A tool's result, clipped to its head and tail.
    Echo,
    /// A subagent's report.
    Work,
    /// What the chat itself records: a rewind that took messages back.
    Note,
}

/// One message of the chat.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct Entry {
    /// The message's permanent id: its place in the chat.
    pub i: u64,
    pub kind: Kind,
    /// The message word for word.
    pub text: String,
    /// Its UTF-8 bytes.
    pub size: usize,
    /// When it entered the log, `YYYY-MM-DDTHH:MM:SSZ`.
    pub date: String,
}

impl Entry {
    pub fn new(i: u64, kind: Kind, text: String, date: String) -> Self {
        Self {
            i,
            kind,
            size: text.len(),
            text,
            date,
        }
    }

    /// The message as its leaf holds it when it fits: `kind: text`.
    pub fn labelled(&self) -> Result<String, String> {
        let kind = serde_json::to_value(self.kind)
            .map_err(|error| format!("message {}'s kind cannot be written: {error}", self.i))?;
        let kind = kind
            .as_str()
            .ok_or_else(|| format!("message {}'s kind is not written as a word", self.i))?;
        Ok(format!("{kind}: {}", self.text))
    }
}

/// `YYYY-MM-DDTHH:MM:SSZ`: the moment a message entered the log.
const MOMENT: &CStr = c"%Y-%m-%dT%H:%M:%SZ";

/// The moment now, in UTC, from the C library's own calendar.
pub fn now() -> Result<String, String> {
    let seconds = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_err(|error| format!("the clock stands before 1970: {error}"))?
        .as_secs();
    let seconds = libc::time_t::try_from(seconds).map_err(|error| {
        format!("the time {seconds} does not fit the C library's time: {error}")
    })?;
    // SAFETY: `tm` is plain data; gmtime_r fills it or answers null.
    let mut tm: libc::tm = unsafe { std::mem::zeroed() };
    if unsafe { libc::gmtime_r(&seconds, &mut tm) }.is_null() {
        return Err(format!(
            "the C library could not place {seconds} on the calendar: {}",
            std::io::Error::last_os_error()
        ));
    }
    // Every field of the pattern expands to at most twice its own length.
    let pattern = MOMENT.to_bytes().len();
    let mut buffer: Vec<u8> = Vec::with_capacity(pattern + pattern);
    // SAFETY: the buffer's capacity is writable and the pattern is NUL
    // terminated; strftime writes at most the capacity and answers how many
    // bytes it wrote, which become the buffer's length.
    unsafe {
        let written = libc::strftime(
            buffer.as_mut_ptr().cast(),
            buffer.capacity(),
            MOMENT.as_ptr(),
            &tm,
        );
        buffer.set_len(written);
    }
    if buffer.is_empty() {
        return Err("the C library could not write the date".to_string());
    }
    String::from_utf8(buffer).map_err(|error| format!("the date is not text: {error}"))
}

/// The day of a `date`: what its file is named after.
pub fn day(date: &str) -> Result<&str, String> {
    date.split_once('T')
        .map(|(day, _)| day)
        .ok_or_else(|| format!("`{date}` is not a date of the chat log"))
}

/// Append `lines` to the file of `day` in `dir`, flushed to disk before the
/// call answers. The log and the tree both keep their lines this way.
pub fn append_lines(dir: &Path, day: &str, lines: &str, what: &str) -> Result<(), String> {
    fs::create_dir_all(dir).map_err(|error| {
        format!(
            "the {what} directory {} cannot be created: {error}",
            dir.display()
        )
    })?;
    let path = dir.join(format!("{day}.jsonl"));
    let mut file = OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)
        .map_err(|error| format!("the {what} {} cannot be opened: {error}", path.display()))?;
    file.write_all(lines.as_bytes())
        .and_then(|()| file.sync_all())
        .map_err(|error| {
            format!(
                "the {what} {} cannot be appended to: {error}",
                path.display()
            )
        })
}

/// Every line of every day file in `dir`, oldest day first. No directory is
/// no lines; a file that cannot be read is refused naming it.
pub fn read_lines(dir: &Path, what: &str) -> Result<Vec<(PathBuf, String)>, String> {
    let listing = match fs::read_dir(dir) {
        Ok(listing) => listing,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => {
            return Err(format!(
                "the {what} directory {} cannot be read: {error}",
                dir.display()
            ))
        }
    };
    let mut files = Vec::new();
    for item in listing {
        let path = item
            .map_err(|error| {
                format!(
                    "the {what} directory {} cannot be listed: {error}",
                    dir.display()
                )
            })?
            .path();
        if path.extension().and_then(|extension| extension.to_str()) == Some("jsonl") {
            files.push(path);
        }
    }
    files.sort();
    let mut lines = Vec::new();
    for path in files {
        let text = fs::read_to_string(&path)
            .map_err(|error| format!("the {what} {} cannot be read: {error}", path.display()))?;
        lines.extend(
            text.lines()
                .filter(|line| !line.trim().is_empty())
                .map(|line| (path.clone(), line.to_string())),
        );
    }
    Ok(lines)
}

/// The append-only log of one chat, under `main/`.
pub struct Log {
    dir: PathBuf,
}

impl Log {
    pub fn at(memory: &Path) -> Self {
        Self {
            dir: memory.join("main"),
        }
    }

    pub fn dir(&self) -> &Path {
        &self.dir
    }

    /// Every message, in order. A line that is not a message, or a message
    /// out of order, is refused naming where it stands, never skipped.
    pub fn read(&self) -> Result<Vec<Entry>, String> {
        let mut entries: Vec<Entry> = Vec::new();
        for (path, line) in read_lines(&self.dir, "chat log")? {
            let entry: Entry = serde_json::from_str(&line).map_err(|error| {
                format!(
                    "the chat log {} holds, after message {}, a line that is not a message ({error})",
                    path.display(),
                    entries.len()
                )
            })?;
            if entry.i != entries.len() as u64 {
                return Err(format!(
                    "the chat log {} holds message {} where message {} belongs",
                    path.display(),
                    entry.i,
                    entries.len()
                ));
            }
            entries.push(entry);
        }
        Ok(entries)
    }

    /// Append `entries` at the end, each in the file of its own day.
    pub fn append(&self, entries: &[Entry]) -> Result<(), String> {
        for entry in entries {
            let line = serde_json::to_string(entry)
                .map_err(|error| format!("message {} cannot be encoded: {error}", entry.i))?;
            append_lines(
                &self.dir,
                day(&entry.date)?,
                &format!("{line}\n"),
                "chat log",
            )?;
        }
        Ok(())
    }
}
