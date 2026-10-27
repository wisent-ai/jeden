//! The hill-climb's memory: one JSON line per round, appended when the round
//! ends, so the next round's choice sees what was chosen and how it ended,
//! and a climb stopped and started again goes on from where it stood.
//!
//! The ledger of a scope lives beside the user configuration, under
//! `hillclimb/<sha256 of the scope's canonical path>.jsonl`, unless `--ledger`
//! names another file. A line that is not a round is refused naming its line,
//! never skipped: a ledger the climb cannot read is a climb that forgets.

use std::fs::{File, OpenOptions, TryLockError};
use std::io::Write;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

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
    let canonical = root
        .canonicalize()
        .map_err(|error| format!("the hill-climb scope {} cannot be resolved: {error}", root.display()))?;
    if let Some(selected) = selected {
        if selected.to_str().is_some_and(|path| path.trim().is_empty()) {
            return Err("the hill-climb ledger needs a file path, not a blank value".into());
        }
        return Ok(canonical.join(selected));
    }
    let digest = hex::encode(Sha256::digest(canonical.to_string_lossy().as_bytes()));
    match crate::user_config_path().parent() {
        Some(directory) => Ok(directory.join("hillclimb").join(format!("{digest}.jsonl"))),
        None => Err("the user configuration path has no directory to keep the hill-climb ledger beside".into()),
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
        std::fs::create_dir_all(directory)
            .map_err(|error| format!("the hill-climb lock directory {} cannot be made: {error}", directory.display()))?;
    }
    let file = OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .open(&lock)
        .map_err(|error| format!("the hill-climb lock {} cannot be opened: {error}", lock.display()))?;
    match file.try_lock() {
        Ok(()) => Ok(file),
        Err(TryLockError::WouldBlock) => Err(format!(
            "another hill-climb owns ledger {} (lock {}); this climb has not chosen or executed a round",
            path.display(), lock.display()
        )),
        Err(TryLockError::Error(error)) => Err(format!(
            "the hill-climb lock {} cannot be locked: {error}", lock.display()
        )),
    }
}

/// Every round the ledger at `path` holds, oldest first; none when the file
/// does not exist yet.
pub(crate) fn read(path: &Path) -> Result<Vec<Round>, String> {
    let text = match std::fs::read_to_string(path) {
        Ok(text) => text,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => return Err(format!("the hill-climb ledger {} cannot be read: {error}", path.display())),
    };
    let mut rounds = Vec::new();
    // Lines are counted as an editor counts them, from the first.
    for (number, line) in (std::iter::once(()).count()..).zip(text.lines()) {
        if line.trim().is_empty() {
            continue;
        }
        let round = serde_json::from_str::<Round>(line).map_err(|error| {
            format!("line {number} of the hill-climb ledger {} is not a round: {error}", path.display())
        })?;
        rounds.push(round);
    }
    Ok(rounds)
}

/// `round` appended to the ledger at `path`, its directory made when missing.
pub(crate) fn append(path: &Path, round: &Round) -> Result<(), String> {
    if let Some(directory) = path.parent() {
        std::fs::create_dir_all(directory)
            .map_err(|error| format!("the hill-climb ledger directory {} cannot be made: {error}", directory.display()))?;
    }
    let line = serde_json::to_string(round).map_err(|error| format!("round {} cannot be written: {error}", round.round))?;
    let mut file = OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
        .map_err(|error| format!("the hill-climb ledger {} cannot be opened: {error}", path.display()))?;
    writeln!(file, "{line}").map_err(|error| format!("the hill-climb ledger {} cannot be written: {error}", path.display()))
}

/// The ledger as the next round's chooser reads it: one line per round.
pub(crate) fn digest(rounds: &[Round]) -> String {
    rounds
        .iter()
        .map(|round| {
            let how = match (&round.ending, &round.summary, &round.error) {
                (Ending::Accepted, Some(summary), _) => format!("accepted: {summary}"),
                (Ending::Failed, _, Some(error)) => format!("failed: {error}"),
                (ending, _, _) => crate::vocabulary::spelling(*ending),
            };
            let place = match &round.repository {
                Some(repository) => format!(" in {repository}"),
                None => String::new(),
            };
            format!("- round {}{place}: {} — {how}", round.round, round.objective)
        })
        .collect::<Vec<_>>()
        .join("\n")
}
