//! The hill-climb's memory: one JSON line per round, appended when the round
//! ends, so the next round's choice sees what was chosen and how it ended,
//! and a climb stopped and started again goes on from where it stood.
//!
//! The ledger of a scope lives beside the user configuration, under
//! `hillclimb/<sha256 of the scope's canonical path>.jsonl`, unless `--ledger`
//! names another file. A line that is not a round is refused naming its line,
//! never skipped: a ledger the climb cannot read is a climb that forgets.

mod identity;

use std::fmt;
use std::fs::{File, OpenOptions, TryLockError};
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

/// Preserve familiar Unicode labels while escaping non-Unicode native units.
pub(crate) struct PathLabel<'a>(pub(crate) &'a Path);

impl fmt::Display for PathLabel<'_> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.0.to_str() {
            Some(unicode) => formatter.write_str(unicode),
            None => fmt::Debug::fmt(self.0, formatter),
        }
    }
}

/// How a round ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub(crate) enum Ending {
    /// Pursuit's independent review accepted the change.
    Accepted,
    /// Pursuit stopped without an accepted change; `error` says why.
    Failed,
}

/// One round: what was chosen, why, and how it ended.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct Round {
    pub(crate) round: u64,
    pub(crate) started_at_ms: u64,
    pub(crate) ended_at_ms: u64,
    pub(crate) objective: String,
    pub(crate) repository: Option<String>,
    pub(crate) kind: Option<String>,
    pub(crate) why: Option<String>,
    pub(crate) ending: Ending,
    pub(crate) summary: Option<String>,
    pub(crate) error: Option<String>,
    pub(crate) receipt: Option<PathBuf>,
    pub(crate) run_directory: Option<PathBuf>,
}

/// The selected ledger; relative overrides belong to the selected workspace.
pub(crate) fn path(root: &Path, selected: Option<&Path>) -> Result<PathBuf, String> {
    let canonical = root.canonicalize().map_err(|error| {
        format!(
            "the hill-climb scope {} cannot be resolved: {error}",
            PathLabel(root)
        )
    })?;
    if let Some(selected) = selected {
        if selected.to_str().is_some_and(|path| path.trim().is_empty()) {
            return Err("the hill-climb ledger needs a file path, not a blank value".into());
        }
        return Ok(canonical.join(selected));
    }
    match crate::user_config_path().parent() {
        Some(directory) => identity::path(directory, &canonical),
        None => Err(
            "the user configuration path has no directory to keep the hill-climb ledger beside"
                .into(),
        ),
    }
}

/// One writer owns a ledger for its whole climb. The sidecar stays on disk:
/// unlinking it would let another process lock a different inode at that name.
/// Reads do not lock it, so recorded rounds remain visible during a climb.
pub(crate) fn claim(path: &Path) -> Result<File, String> {
    let suffix = ".lock";
    let mut name = std::ffi::OsString::with_capacity(path.as_os_str().len() + suffix.len());
    name.push(path.as_os_str());
    name.push(suffix);
    let lock = PathBuf::from(name);
    if let Some(directory) = lock.parent() {
        std::fs::create_dir_all(directory).map_err(|error| {
            format!(
                "the hill-climb lock directory {} cannot be made: {error}",
                PathLabel(directory)
            )
        })?;
    }
    let file = OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .open(&lock)
        .map_err(|error| {
            format!(
                "the hill-climb lock {} cannot be opened: {error}",
                PathLabel(&lock)
            )
        })?;
    match file.try_lock() {
        Ok(()) => Ok(file),
        Err(TryLockError::WouldBlock) => Err(format!(
            "another hill-climb owns ledger {} (lock {}); this climb has not chosen or executed a round",
            PathLabel(path), PathLabel(&lock)
        )),
        Err(TryLockError::Error(error)) => Err(format!(
            "the hill-climb lock {} cannot be locked: {error}", PathLabel(&lock)
        )),
    }
}

/// Every round the ledger at `path` holds, oldest first; none when the file
/// does not exist yet. Reuse one record buffer rather than retaining a second,
/// whole-file string beside the decoded history.
pub(crate) fn read(path: &Path) -> Result<Vec<Round>, String> {
    let mut rounds = Vec::new();
    visit(path, |round| rounds.push(round))?;
    Ok(rounds)
}

/// Decode every physical record, even when a consumer keeps only a suffix.
pub(crate) fn visit(path: &Path, mut accept: impl FnMut(Round)) -> Result<(), String> {
    let file = match File::open(path) {
        Ok(file) => file,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(error) => {
            return Err(format!(
                "the hill-climb ledger {} cannot be read: {error}",
                PathLabel(path)
            ))
        }
    };
    let mut reader = BufReader::new(file);
    let mut line = String::new();
    // Lines are counted as an editor counts them, from the first.
    for number in std::iter::once(()).count().. {
        line.clear();
        reader.read_line(&mut line).map_err(|error| {
            format!(
                "line {number} of the hill-climb ledger {} cannot be read: {error}",
                PathLabel(path)
            )
        })?;
        if line.is_empty() {
            break;
        }
        // Remove one LF and its optional CR, not arbitrary trailing control
        // bytes that still belong to the record's JSON validation.
        if line.ends_with('\n') {
            let _ = line.pop();
            if line.ends_with('\r') {
                let _ = line.pop();
            }
        }
        if line.trim().is_empty() {
            continue;
        }
        let round = serde_json::from_str::<Round>(&line).map_err(|error| {
            format!(
                "line {number} of the hill-climb ledger {} is not a round: {error}",
                PathLabel(path)
            )
        })?;
        accept(round);
    }
    Ok(())
}

/// `round` appended to the ledger at `path`, its directory made when missing.
pub(crate) fn append(path: &Path, round: &Round) -> Result<(), String> {
    if let Some(directory) = path.parent() {
        std::fs::create_dir_all(directory).map_err(|error| {
            format!(
                "the hill-climb ledger directory {} cannot be made: {error}",
                PathLabel(directory)
            )
        })?;
    }
    let line = serde_json::to_string(round)
        .map_err(|error| format!("round {} cannot be written: {error}", round.round))?;
    let mut file = OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
        .map_err(|error| {
            format!(
                "the hill-climb ledger {} cannot be opened: {error}",
                PathLabel(path)
            )
        })?;
    writeln!(file, "{line}").map_err(|error| {
        format!(
            "the hill-climb ledger {} cannot be written: {error}",
            PathLabel(path)
        )
    })
}

/// Borrowed history: status and progress format it into their existing message;
/// only the chooser, whose template needs a string, materializes one.
pub(crate) fn digest(rounds: &[Round]) -> impl fmt::Display + '_ {
    Digest(rounds)
}

struct Digest<'a>(&'a [Round]);

impl fmt::Display for Digest<'_> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let mut first = true;
        for round in self.0 {
            if !first {
                formatter.write_str("\n")?;
            }
            first = false;
            write!(formatter, "- round {}", round.round)?;
            if let Some(repository) = &round.repository {
                formatter.write_str(" in ")?;
                formatter.write_str(repository)?;
            }
            formatter.write_str(": ")?;
            formatter.write_str(&round.objective)?;
            formatter.write_str(" — ")?;
            match (&round.ending, &round.summary, &round.error) {
                (Ending::Accepted, Some(summary), _) => {
                    formatter.write_str("accepted: ")?;
                    formatter.write_str(summary)?;
                }
                (Ending::Failed, _, Some(error)) => {
                    formatter.write_str("failed: ")?;
                    formatter.write_str(error)?;
                }
                (Ending::Accepted, _, _) => formatter.write_str("accepted")?,
                (Ending::Failed, _, _) => formatter.write_str("failed")?,
            }
        }
        Ok(())
    }
}
