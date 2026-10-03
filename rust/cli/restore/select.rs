//! Which agent sessions the operator was working in: every OMP transcript
//! with a message the operator wrote at or after a given instant. The
//! transcripts answer this, never terminal-device files, which keep one path
//! per terminal and are overwritten when the device is reused.

use serde::Deserialize;
use std::fs::{self, File};
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::time::UNIX_EPOCH;

pub(super) struct Selected {
    pub id: String,
    pub transcript: PathBuf,
    pub cwd: String,
    pub title: String,
    pub last_operator_message: i64,
}

pub(super) struct Unreadable {
    pub transcript: PathBuf,
    pub error: String,
}

pub(super) struct Scan {
    pub sessions: Vec<Selected>,
    pub unreadable: Vec<Unreadable>,
}

/// The session header of one transcript: what `omp --resume` needs.
pub(super) struct Header {
    pub id: String,
    pub cwd: String,
}

/// One JSONL record, read only for the fields selection needs; every other
/// field, tool output included, is skipped by the parser.
#[derive(Deserialize)]
struct Record {
    #[serde(rename = "type")]
    kind: Option<String>,
    id: Option<String>,
    cwd: Option<String>,
    title: Option<String>,
    timestamp: Option<String>,
    message: Option<Message>,
}

#[derive(Deserialize)]
struct Message {
    role: Option<String>,
    attribution: Option<String>,
}

impl Record {
    /// OMP records what the operator typed as a user message attributed to
    /// the user; messages a harness or agent injects carry another
    /// attribution, and custom messages are another record type.
    fn operator_instant(&self) -> Option<i64> {
        let message = self.message.as_ref()?;
        if self.kind.as_deref() != Some("message")
            || message.role.as_deref() != Some("user")
            || message.attribution.as_deref() != Some("user")
        {
            return None;
        }
        super::clock::utc(self.timestamp.as_deref()?)
    }
}

/// OMP keeps `<root>/<workspace>/<session>.jsonl`; deeper files are the
/// transcripts of subagents a session spawned, which are not sessions the
/// operator wrote in, and dot files are OMP's locks.
pub(super) fn scan(root: &Path, since: i64) -> Result<Scan, String> {
    let workspaces = fs::read_dir(root)
        .map_err(|error| format!("cannot read the OMP session root {}: {error}", root.display()))?;
    let mut scan = Scan { sessions: Vec::new(), unreadable: Vec::new() };
    for workspace in workspaces {
        let workspace = workspace.map_err(|error| format!("cannot read {}: {error}", root.display()))?;
        let path = workspace.path();
        if hidden(&path) || !path.is_dir() {
            continue;
        }
        let entries = fs::read_dir(&path)
            .map_err(|error| format!("cannot read {}: {error}", path.display()))?;
        for entry in entries {
            let entry = entry.map_err(|error| format!("cannot read {}: {error}", path.display()))?;
            let transcript = entry.path();
            if hidden(&transcript)
                || transcript.extension().and_then(|ext| ext.to_str()) != Some("jsonl")
                || !transcript.is_file()
            {
                continue;
            }
            // A transcript last written before `since` cannot hold a message
            // written after it.
            match modified_millis(&transcript) {
                Ok(modified) if modified < since => continue,
                Ok(_) => {}
                Err(error) => {
                    scan.unreadable.push(Unreadable { transcript, error });
                    continue;
                }
            }
            match read(&transcript, since) {
                Ok(Some(session)) => scan.sessions.push(session),
                Ok(None) => {}
                Err(error) => scan.unreadable.push(Unreadable { transcript, error }),
            }
        }
    }
    scan.sessions.sort_by(|a, b| a.last_operator_message.cmp(&b.last_operator_message));
    Ok(scan)
}

/// The header alone, for `jeden restore open`.
pub(super) fn header(transcript: &Path) -> Result<Header, String> {
    let mut found = None;
    each_record(transcript, |record| {
        if record.kind.as_deref() == Some("session") {
            found = Some(Header {
                id: record.id.clone().unwrap_or_default(),
                cwd: record.cwd.clone().unwrap_or_default(),
            });
            return false;
        }
        true
    })?;
    match found {
        Some(header) if !header.id.is_empty() && !header.cwd.is_empty() => Ok(header),
        Some(_) => Err(format!("{}: its session header names no id or no workspace", transcript.display())),
        None => Err(format!("{}: not an OMP transcript (no session header)", transcript.display())),
    }
}

fn read(transcript: &Path, since: i64) -> Result<Option<Selected>, String> {
    let mut header = None;
    let mut title = None;
    let mut last = None;
    each_record(transcript, |record| {
        match record.kind.as_deref() {
            Some("session") => {
                header = Some((record.id.clone(), record.cwd.clone()));
                if title.is_none() {
                    title = record.title.clone();
                }
            }
            Some("title") | Some("title_change") if record.title.is_some() => title = record.title.clone(),
            _ => {}
        }
        if let Some(instant) = record.operator_instant() {
            if instant >= since {
                last = Some(instant);
            }
        }
        true
    })?;
    let Some(last_operator_message) = last else {
        return Ok(None);
    };
    let (Some(id), Some(cwd)) = header.unwrap_or_default() else {
        return Err("operator messages but no session header naming its id and workspace".into());
    };
    Ok(Some(Selected {
        title: title.unwrap_or_else(|| id.clone()),
        id,
        transcript: transcript.to_path_buf(),
        cwd,
        last_operator_message,
    }))
}

/// Calls `visit` for every record until it answers false. A final line with
/// no newline is a record OMP is still writing and ends the read; any other
/// line that is not a JSON record makes the transcript unreadable, named by
/// its line number.
fn each_record(transcript: &Path, mut visit: impl FnMut(&Record) -> bool) -> Result<(), String> {
    let file = File::open(transcript).map_err(|error| format!("cannot open: {error}"))?;
    let mut reader = BufReader::new(file);
    let mut line = String::new();
    let mut number = 0usize;
    loop {
        line.clear();
        if reader.read_line(&mut line).map_err(|error| format!("cannot read: {error}"))? == 0 {
            return Ok(());
        }
        number += 1;
        if line.trim().is_empty() {
            continue;
        }
        let record: Record = match serde_json::from_str(&line) {
            Ok(record) => record,
            Err(_) if !line.ends_with('\n') => return Ok(()),
            Err(error) => return Err(format!("line {number} is not a JSON record: {error}")),
        };
        if !visit(&record) {
            return Ok(());
        }
    }
}

fn hidden(path: &Path) -> bool {
    path.file_name()
        .and_then(|name| name.to_str())
        .is_some_and(|name| name.starts_with('.'))
}

fn modified_millis(path: &Path) -> Result<i64, String> {
    let modified = fs::metadata(path)
        .and_then(|metadata| metadata.modified())
        .map_err(|error| format!("cannot read its modification time: {error}"))?;
    let since_epoch = modified
        .duration_since(UNIX_EPOCH)
        .map_err(|error| format!("its modification time precedes the epoch: {error}"))?;
    i64::try_from(since_epoch.as_millis()).map_err(|error| error.to_string())
}
