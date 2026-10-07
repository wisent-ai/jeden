//! Omp sessions answering under contracts older than the installed block.
//!
//! Omp reads `APPEND_SYSTEM.md` once, when a session starts, and keeps that
//! system prompt for the session's life. A block installed later reaches only
//! the sessions started after it, so a file that reads `current` said nothing
//! about the sessions the operator was talking to: one kept writing the
//! headed reports his new contract forbade. `status --omp` names every
//! session that started before the file was last written and has written to
//! its transcript since; each of those turns ran under the text the file held
//! before.

use std::fs;
use std::io::ErrorKind;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

/// Where Omp keeps session transcripts: `sessions/` beside the file it
/// appends to every system prompt.
pub(super) fn omp_sessions_root(append_system: &Path) -> Result<PathBuf, String> {
    append_system
        .parent()
        .map(|agent| agent.join("sessions"))
        .ok_or_else(|| format!("{} has no parent directory", append_system.display()))
}

/// Every transcript under `root` that started before `append_system` was last
/// written and was written after it, sorted by path.
pub(super) fn on_older_text(append_system: &Path, root: &Path) -> Result<Vec<PathBuf>, String> {
    let written = fs::metadata(append_system)
        .and_then(|metadata| metadata.modified())
        .map_err(|error| {
            format!(
                "cannot read when {} was written: {error}",
                append_system.display()
            )
        })?;
    let mut transcripts = Vec::new();
    walk(root, &mut transcripts)?;
    let mut older = Vec::new();
    for transcript in transcripts {
        let (started, active) = times(&transcript)?;
        if started < written && active > written {
            older.push(transcript);
        }
    }
    older.sort();
    Ok(older)
}

/// When the transcript was created (the session's start) and last written.
fn times(transcript: &Path) -> Result<(SystemTime, SystemTime), String> {
    let metadata = fs::metadata(transcript)
        .map_err(|error| format!("cannot read {}: {error}", transcript.display()))?;
    let started = metadata.created().map_err(|error| {
        format!(
            "{}: the file system records no creation time ({error}), so it cannot be said \
             whether this session started before the contracts were installed",
            transcript.display()
        )
    })?;
    let active = metadata.modified().map_err(|error| {
        format!(
            "cannot read when {} was last written: {error}",
            transcript.display()
        )
    })?;
    Ok((started, active))
}

/// Every `.jsonl` file below `directory`. A missing directory is a machine
/// where Omp never ran a session, which holds none.
fn walk(directory: &Path, found: &mut Vec<PathBuf>) -> Result<(), String> {
    let entries = match fs::read_dir(directory) {
        Ok(entries) => entries,
        Err(error) if error.kind() == ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(format!("cannot list {}: {error}", directory.display())),
    };
    for entry in entries {
        let path = entry
            .map_err(|error| format!("cannot list {}: {error}", directory.display()))?
            .path();
        if path.is_dir() {
            walk(&path, found)?;
        } else if path
            .extension()
            .is_some_and(|extension| extension == "jsonl")
        {
            found.push(path);
        }
    }
    Ok(())
}
