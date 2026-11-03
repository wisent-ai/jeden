//! The request for a GitHub star: after the operator's fifth successful
//! prompt Jeden asks, once, for a star on its public repository. The count
//! and the answer are kept per user in `~/.jeden/star.json`, so every
//! surface (terminal, `jeden run`, RPC and Jeden Desktop) counts into the
//! same number and the request is made once, wherever the fifth prompt was
//! sent. `jeden star` reads, dismisses and resets it.

pub(crate) mod command;

use serde::{Deserialize, Serialize};
use std::fs::{File, OpenOptions};
use std::path::PathBuf;
use std::sync::LazyLock;

/// The public repository the request links to, from the package manifest.
pub(crate) const REPOSITORY: &str = env!("CARGO_PKG_REPOSITORY");

#[derive(Deserialize)]
struct Declared<T> {
    value: T,
}

#[derive(Deserialize)]
struct Numbers {
    prompts_before_request: Declared<u64>,
}

static NUMBERS: LazyLock<Numbers> = LazyLock::new(|| {
    serde_json::from_str(include_str!("numeric-provenance.json"))
        .expect("numeric-provenance.json declares the operator's star request threshold")
});

/// How many successful prompts are sent before the request is made.
pub(crate) fn threshold() -> u64 {
    NUMBERS.prompts_before_request.value
}

/// What is kept: the successful prompts counted, and when the request was
/// shown and dismissed, in whole seconds since the Unix epoch.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Record {
    prompts: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    shown_at: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    dismissed_at: Option<u64>,
}

/// Where the request stands.
#[derive(Debug, Clone, Copy, Serialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub(crate) enum Stage {
    /// Fewer successful prompts than the threshold.
    Counting,
    /// The threshold is reached and no surface has shown the request yet.
    Due,
    /// A surface showed the request and it was not dismissed.
    Shown,
    /// The operator dismissed it; it is never shown again until a reset.
    Dismissed,
}

/// The request's state as every surface reads it.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub(crate) struct Status {
    pub stage: Stage,
    pub prompts: u64,
    pub threshold: u64,
    pub repository: &'static str,
    pub file: String,
    pub shown_at: Option<u64>,
    pub dismissed_at: Option<u64>,
}

fn state_path() -> PathBuf {
    crate::slash::common::dirs_home().join(".jeden/star.json")
}

fn status_of(record: &Record) -> Status {
    let stage = if record.dismissed_at.is_some() {
        Stage::Dismissed
    } else if record.shown_at.is_some() {
        Stage::Shown
    } else if record.prompts >= threshold() {
        Stage::Due
    } else {
        Stage::Counting
    };
    Status {
        stage,
        prompts: record.prompts,
        threshold: threshold(),
        repository: REPOSITORY,
        file: state_path().display().to_string(),
        shown_at: record.shown_at,
        dismissed_at: record.dismissed_at,
    }
}

fn now() -> Result<u64, String> {
    crate::clock::unix_seconds().map_err(|error| error.to_string())
}

/// The record on disk; a file that does not exist yet is a record of
/// nothing. One that cannot be read or is not a star record is refused
/// naming the file, the cause and how to start over.
fn read(path: &PathBuf) -> Result<Record, String> {
    match std::fs::read(path) {
        Ok(bytes) => serde_json::from_slice(&bytes).map_err(|error| {
            format!(
                "{} is not a star record ({error}); `jeden star reset` writes a new one",
                path.display()
            )
        }),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(Record::default()),
        Err(error) => Err(format!("{} cannot be read: {error}", path.display())),
    }
}

fn write(path: &PathBuf, record: &Record) -> Result<(), String> {
    let mut bytes = serde_json::to_vec_pretty(record)
        .map_err(|error| format!("the star record cannot be written as JSON: {error}"))?;
    bytes.push(b'\n');
    crate::tool_runtime::runtime_ops::durable::replace(path, &bytes)
}

/// Hold the star record for this process while it reads and replaces it,
/// so two Jeden processes finishing a prompt at once both count.
fn lock(path: &PathBuf) -> Result<File, String> {
    use std::os::unix::io::AsRawFd;
    let dir = path
        .parent()
        .ok_or_else(|| format!("{} has no parent directory", path.display()))?;
    std::fs::create_dir_all(dir)
        .map_err(|error| format!("{} cannot be created: {error}", dir.display()))?;
    let lock_path = path.with_extension("lock");
    let file = OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .open(&lock_path)
        .map_err(|error| format!("{} cannot be opened: {error}", lock_path.display()))?;
    stado_wait::blocking(
        stado_wait::Kind::Lock,
        lock_path.display(),
        "this machine",
        || {
            // SAFETY: flock on a descriptor this process owns; the lock ends
            // when the returned file is dropped.
            let locked = unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX) };
            if locked == libc::EXIT_SUCCESS {
                Ok(())
            } else {
                Err(std::io::Error::last_os_error())
            }
        },
    )
    .map_err(|error| format!("{} cannot be locked: {error}", lock_path.display()))?;
    Ok(file)
}

/// Change the record under the lock and answer the status it leaves.
fn change<T>(apply: impl FnOnce(&mut Record) -> T) -> Result<(T, Status), String> {
    let path = state_path();
    let _held = lock(&path)?;
    let mut record = read(&path)?;
    let answer = apply(&mut record);
    write(&path, &record)?;
    Ok((answer, status_of(&record)))
}

/// The request's current state, without changing it.
pub(crate) fn status() -> Result<Status, String> {
    read(&state_path()).map(|record| status_of(&record))
}

/// Count one successful prompt. The status is answered as the request to
/// show when this prompt makes it due: it is then recorded as shown, so no
/// other surface makes it again.
pub(crate) fn observe_prompt() -> Result<Option<Status>, String> {
    let stamp = now()?;
    let (shown, status) = change(|record| {
        record.prompts = record.prompts.saturating_add(1);
        let due = record.prompts >= threshold()
            && record.shown_at.is_none()
            && record.dismissed_at.is_none();
        if due {
            record.shown_at = Some(stamp);
        }
        due
    })?;
    Ok(shown.then_some(status))
}

/// Count one successful prompt from a caller that cannot fail on the
/// bookkeeping: a record that cannot be kept is said on standard error,
/// naming the cause, and the prompt's own answer stands.
pub(crate) fn observe_prompt_reporting() -> Option<Status> {
    match observe_prompt() {
        Ok(request) => request,
        Err(cause) => {
            eprintln!("jeden: this prompt is not counted toward the GitHub star request: {cause}");
            None
        }
    }
}

/// Never show the request again, until a reset.
pub(crate) fn dismiss() -> Result<Status, String> {
    let stamp = now()?;
    change(|record| {
        if record.dismissed_at.is_none() {
            record.dismissed_at = Some(stamp);
        }
    })
    .map(|((), status)| status)
}

/// Start over: the count returns to zero and the request is made again
/// after the threshold. A record that cannot be read is replaced.
pub(crate) fn reset() -> Result<Status, String> {
    let path = state_path();
    let _held = lock(&path)?;
    let record = Record::default();
    write(&path, &record)?;
    Ok(status_of(&record))
}

/// The request in words, in the workspace's interface language.
pub(crate) fn request_text(lang: &str, status: &Status) -> String {
    crate::cli::i18n::tr(lang, "star.request")
        .replace("{prompts}", &status.prompts.to_string())
        .replace("{repository}", status.repository)
}

/// When a recorded moment was, as the record keeps it.
fn moment(seconds: Option<u64>) -> String {
    match seconds {
        Some(seconds) => format!("at {seconds} (Unix seconds)"),
        None => "at a moment the record does not hold".into(),
    }
}

/// The status in words, one line per fact.
pub(crate) fn status_text(status: &Status) -> String {
    let stage = match status.stage {
        Stage::Counting => format!(
            "counting: {} of {} successful prompts; the request is made at {}",
            status.prompts, status.threshold, status.threshold
        ),
        Stage::Due => format!(
            "due: {} successful prompts; the next successful prompt shows the request",
            status.prompts
        ),
        Stage::Shown => format!(
            "shown {}; {} successful prompts; `jeden star dismiss` stops it",
            moment(status.shown_at),
            status.prompts
        ),
        Stage::Dismissed => format!(
            "dismissed {}; {} successful prompts; `jeden star reset` asks again after {}",
            moment(status.dismissed_at),
            status.prompts,
            status.threshold
        ),
    };
