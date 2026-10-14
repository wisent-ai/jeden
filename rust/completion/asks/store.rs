//! `operator-asks.json` beside the session root: one document, locked for
//! every change and replaced atomically.

use super::{AskOccurrence, AskRegister, AskSource, OperatorAsk, SCHEMA_VERSION};
use crate::completion::store::{regular_file_or_absent, write_atomic, STATE_FILE};
use crate::completion::CompletionState;
use std::fs::{self, OpenOptions};
use std::path::{Path, PathBuf};

const REGISTER_FILE: &str = "operator-asks.json";
const LOCK_FILE: &str = "operator-asks.lock";

/// The register sits beside the session root it records, so a Jeden whose
/// sessions live elsewhere (`JEDEN_SESSION_ROOT`) keeps its asks there too.
pub(crate) fn path() -> PathBuf {
    crate::session_root().with_file_name(REGISTER_FILE)
}

/// The register, or, before it was ever written, the asks already recorded
/// on tasks of this machine's sessions, adopted once and written.
pub(crate) fn read() -> Result<AskRegister, String> {
    let file = path();
    regular_file_or_absent(&file)?;
    match fs::read(&file) {
        Ok(bytes) => parse(&file, &bytes),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            update(|_| Ok(())).map(|(_, register)| register)
        }
        Err(error) => Err(format!(
            "cannot read the operator ask register {}: {error}",
            file.display()
        )),
    }
}

fn parse(file: &Path, bytes: &[u8]) -> Result<AskRegister, String> {
    let register: AskRegister = serde_json::from_slice(bytes)
        .map_err(|error| format!("invalid operator ask register {}: {error}", file.display()))?;
    if register.schema_version != SCHEMA_VERSION {
        return Err(format!(
            "unsupported operator ask register version {} in {}",
            register.schema_version,
            file.display()
        ));
    }
    // Every reader orders asks and occurrences by these times; one that is not
    // milliseconds since the epoch is refused here, where the file is named,
    // never ordered as the earliest by whichever reader meets it.
    for ask in &register.asks {
        let times = std::iter::once(&ask.asked_at).chain(
            ask.occurrences
                .iter()
                .map(|occurrence| &occurrence.asked_at),
        );
        for time in times {
            time.parse::<u64>().map_err(|error| {
                format!(
                    "{} records ask {} as asked at {time:?}, which is not milliseconds since the epoch: {error}",
                    file.display(),
                    ask.id
                )
            })?;
        }
    }
    Ok(register)
}

pub(crate) fn update<T>(
    change: impl FnOnce(&mut AskRegister) -> Result<T, String>,
) -> Result<(T, AskRegister), String> {
    let file = path();
    let parent = file
        .parent()
        .ok_or_else(|| format!("{} has no parent directory", file.display()))?;
    fs::create_dir_all(parent)
        .map_err(|error| format!("cannot create {}: {error}", parent.display()))?;
    let lock_path = parent.join(LOCK_FILE);
    regular_file_or_absent(&lock_path)?;
    let lock = OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .open(&lock_path)
        .map_err(|error| format!("cannot open {}: {error}", lock_path.display()))?;
    lock.lock()
        .map_err(|error| format!("cannot lock the operator ask register: {error}"))?;
    regular_file_or_absent(&file)?;
    let mut register = match fs::read(&file) {
        Ok(bytes) => parse(&file, &bytes)?,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => adopt()?,
        Err(error) => {
            return Err(format!(
                "cannot read the operator ask register {}: {error}",
                file.display()
            ))
        }
    };
    let output = change(&mut register)?;
    register.revision = register
        .revision
        .checked_add(u64::from(true))
        .ok_or("operator ask register revision overflow")?;
    write_atomic(&file, &register)?;
    Ok((output, register))
}

/// Asks recorded on tasks before the register existed. The same words asked
/// in several sessions become one ask with several occurrences, and an
/// answer given on any of those tasks is that ask's answer.
fn adopt() -> Result<AskRegister, String> {
    let mut register = AskRegister {
        schema_version: SCHEMA_VERSION,
        revision: 0,
        asks: Vec::new(),
    };
    let root = crate::session_root();
    let entries = match fs::read_dir(&root) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(register),
        Err(error) => return Err(format!("cannot read {}: {error}", root.display())),
    };
    // An entry that cannot be read is refused like the root: the register is
    // written once from what is adopted here, so a session skipped now would
    // lose its asks for good.
    let mut sessions: Vec<PathBuf> = Vec::new();
    for entry in entries {
        let dir = entry
            .map_err(|error| format!("cannot read {}: {error}", root.display()))?
            .path();
        if dir.join(STATE_FILE).is_file() {
            sessions.push(dir);
        }
    }
    sessions.sort();
    for session in sessions {
        // The register is written once from what is adopted here, so a state
        // skipped now would lose its asks for good.
        let path = session.join(STATE_FILE);
        let bytes =
            fs::read(&path).map_err(|error| format!("cannot read {}: {error}", path.display()))?;
        let state = serde_json::from_slice::<CompletionState>(&bytes)
            .map_err(|error| format!("{} is not a completion state: {error}", path.display()))?;
        let key = super::session_key(&session);
        for task in &state.tasks {
            let Some(request) = &task.operator_request else {
                continue;
            };
            let workspace = state
                .requests
                .iter()
                .find(|item| item.id == task.request_id)
                .map(|item| item.cwd.clone())
                .unwrap_or_default();
            let occurrence = AskOccurrence {
                asked_at: request.asked_at.clone(),
                source: AskSource::Review,
                session_path: key.clone(),
                task_id: Some(task.id.clone()),
                wording: request.ask.clone(),
                answered_from_register: false,
            };
            match register
                .asks
                .iter_mut()
                .find(|ask| super::same_words(&ask.ask, &request.ask))
            {
                Some(ask) => {
                    ask.occurrences.push(occurrence);
                    if ask.answer.is_none() {
                        ask.answer.clone_from(&request.answer);
                    }
                }
                None => register.asks.push(OperatorAsk {
                    id: uuid::Uuid::new_v4().to_string(),
                    ask: request.ask.clone(),
                    workspace,
                    asked_at: request.asked_at.clone(),
                    follows: None,
                    occurrences: vec![occurrence],
                    answer: request.answer.clone(),
                    oko: None,
                }),
            }
        }
    }
    // Occurrences are ordered by when they were asked; one whose time is not
    // milliseconds since the epoch is refused with where it was read, never
    // ordered as the earliest and made the ask's own time.
    for ask in &mut register.asks {
        let mut stamped = Vec::new();
        for occurrence in ask.occurrences.drain(..) {
            let stamp = occurrence.asked_at.parse::<u64>().map_err(|error| {
                format!(
                    "{:?} records the ask {:?} as asked at {:?}, which is not milliseconds since the epoch: {error}",
                    occurrence.session_path, ask.ask, occurrence.asked_at
                )
            })?;
            stamped.push((stamp, occurrence));
        }
        stamped.sort_by_key(|(stamp, _)| *stamp);
        ask.occurrences = stamped
            .into_iter()
            .map(|(_, occurrence)| occurrence)
            .collect();
        if let Some(first) = ask.occurrences.first() {
            ask.asked_at.clone_from(&first.asked_at);
        }
    }
    Ok(register)
}
