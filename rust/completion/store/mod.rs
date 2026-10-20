mod legacy;
mod workspace;
pub(crate) use workspace::migrate as migrate_workspace;

use super::model::*;
#[cfg(unix)]
use std::fs::File;
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};

pub(crate) const STATE_FILE: &str = "completion.json";
const LOCK_FILE: &str = "completion.lock";

pub(crate) fn regular_file_or_absent(path: &Path) -> Result<(), String> {
    match fs::symlink_metadata(path) {
        Ok(metadata) if metadata.is_file() && !metadata.file_type().is_symlink() => Ok(()),
        Ok(_) => Err(format!(
            "Jeden state path is not a regular file: {}",
            path.display()
        )),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(format!("cannot inspect {}: {error}", path.display())),
    }
}

fn session_directory(dir: &Path) -> Result<(), String> {
    let metadata = fs::symlink_metadata(dir)
        .map_err(|error| format!("cannot inspect session {}: {error}", dir.display()))?;
    if !metadata.is_dir() || metadata.file_type().is_symlink() || !dir.join("state.json").is_file()
    {
        return Err(format!("not a durable Jeden session: {}", dir.display()));
    }
    Ok(())
}

pub(crate) fn read(dir: &Path) -> Result<CompletionState, String> {
    session_directory(dir)?;
    let file = dir.join(STATE_FILE);
    regular_file_or_absent(&file)?;
    let mut state: CompletionState = match fs::read(&file) {
        Ok(bytes) => serde_json::from_slice(&bytes)
            .map_err(|error| format!("invalid completion state {}: {error}", file.display()))?,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => legacy::legacy_state(dir)?,
        Err(error) => {
            return Err(format!(
                "cannot read completion state {}: {error}",
                file.display()
            ))
        }
    };
    if super::constants::UPGRADED_SCHEMA_VERSIONS.contains(&state.schema_version) {
        state.schema_version = super::constants::SCHEMA_VERSION;
    }
    state.validate()?;
    Ok(state)
}

pub(crate) fn update<T>(
    dir: &Path,
    expected_revision: Option<u64>,
    change: impl FnOnce(&mut CompletionState) -> Result<T, String>,
) -> Result<(T, CompletionState), String> {
    session_directory(dir)?;
    let lock_path = dir.join(LOCK_FILE);
    regular_file_or_absent(&lock_path)?;
    let lock = OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .open(&lock_path)
        .map_err(|error| {
            format!(
                "cannot open completion lock {}: {error}",
                lock_path.display()
            )
        })?;
    lock.lock()
        .map_err(|error| format!("cannot lock completion state: {error}"))?;
    let mut state = read(dir)?;
    if let Some(expected) = expected_revision {
        if state.revision != expected {
            return Err(format!(
                "completion state changed: expected revision {expected}, found {}",
                state.revision
            ));
        }
    }
    let output = change(&mut state)?;
    state.revision = state
        .revision
        .checked_add(std::num::NonZeroU64::MIN.get())
        .ok_or("completion revision overflow")?;
    state.validate()?;
    write_atomic(&dir.join(STATE_FILE), &state)?;
    let legacy = dir.join("artifacts/todo.json");
    if legacy.exists() {
        regular_file_or_absent(&legacy)?;
        fs::remove_file(&legacy).map_err(|error| {
            format!("completion state saved but legacy todo retirement failed: {error}")
        })?;
    }
    Ok((output, state))
}

/// Writes `value` as pretty JSON beside `path` and renames it into place, so a
/// reader sees the previous document or the new one, never a torn write.
pub(crate) fn write_atomic<T: serde::Serialize>(path: &Path, value: &T) -> Result<(), String> {
    regular_file_or_absent(path)?;
    let parent = path
        .parent()
        .ok_or_else(|| format!("{} has no parent directory", path.display()))?;
    let staging = parent.join(format!(".completion-{}.new", uuid::Uuid::new_v4()));
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        // Readable and writable by its owner only, as POSIX names the bits.
        options.mode(u32::from(libc::S_IRUSR | libc::S_IWUSR));
    }
    let result: Result<(), String> = (|| {
        let mut file = options.open(&staging).map_err(|error| error.to_string())?;
        serde_json::to_writer_pretty(&mut file, value).map_err(|error| error.to_string())?;
        file.write_all(b"\n").map_err(|error| error.to_string())?;
        file.sync_all().map_err(|error| error.to_string())?;
        fs::rename(&staging, path).map_err(|error| error.to_string())?;
        #[cfg(unix)]
        File::open(parent)
            .and_then(|directory| directory.sync_all())
            .map_err(|error| error.to_string())?;
        Ok(())
    })();
    let Err(error) = result else {
        return Ok(());
    };
    let refused = format!("cannot persist {}: {error}", path.display());
    match fs::remove_file(&staging) {
        Err(left) if left.kind() != std::io::ErrorKind::NotFound => Err(format!(
            "{refused}; {} is left behind: {left}",
            staging.display()
        )),
        _ => Err(refused),
    }
}

pub(crate) fn inherit(source: &Path, destination: &Path) -> Result<CompletionState, String> {
    let inherited = read(source)?;
    let (_, state) = update(destination, None, |state| {
        if !state.requests.is_empty() || !state.tasks.is_empty() {
            return Err("cannot replace an existing session's completion obligations".into());
        }
        state.requests = inherited.requests;
        state.tasks = inherited.tasks;
        state.blocker = inherited.blocker;
        Ok(())
    })?;
    Ok(state)
}

pub(crate) fn session_from_artifacts(path: &Path) -> Result<PathBuf, String> {
    if path.file_name().and_then(|name| name.to_str()) != Some("artifacts") {
        return Err("todo requires the active session artifact directory".into());
    }
    let session = path.parent().ok_or("todo has no owning session")?;
    session_directory(session)?;
    Ok(session.to_path_buf())
}
