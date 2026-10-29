//! Finishing or undoing an update that was interrupted, using the journal the
//! installer wrote before each step.
//!
//! Split out of `update/transaction.rs`, which had grown past the module line
//! cap.

use super::disk::{durable_json, sync_dir};
use super::{InstallPaths, InstalledState, Journal, Phase};
use crate::tool_runtime::runtime_ops::platform::presence::{present, remove_present};
use std::fs;

pub fn recover(paths: &InstallPaths) -> Result<Option<String>, String> {
    let bytes = match fs::read(&paths.journal) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(format!("read update journal: {error}")),
    };
    // A journal in another shape (another release's format included) is
    // refused naming the field serde could not read, never acted on.
    let entry: Journal = serde_json::from_slice(&bytes).map_err(|error| {
        format!(
            "invalid update journal {}: {error}",
            paths.journal.display()
        )
    })?;
    match entry.phase {
        Phase::Prepared | Phase::Staged => remove_present(&paths.stage, "interrupted stage")?,
        Phase::BackingUp | Phase::BackedUp => {
            if present(&paths.backup)? {
                remove_present(&paths.target, "uncertain target")?;
                fs::rename(&paths.backup, &paths.target)
                    .map_err(|error| format!("restore last-known-good: {error}"))?;
            }
            remove_present(&paths.stage, "interrupted stage")?;
        }
        Phase::Activated => {
            rollback(paths)?;
            restore_state(paths, &entry.previous_state)?;
        }
        Phase::Committed => {
            remove_present(&paths.backup, "committed backup")?;
            remove_present(&paths.stage, "committed stage")?;
        }
    }
    sync_dir(paths.parent())?;
    fs::remove_file(&paths.journal)
        .map_err(|error| format!("remove recovered journal: {error}"))?;
    sync_dir(paths.parent())?;
    Ok(Some(format!(
        "recovered interrupted update {} -> {}",
        entry.from_version, entry.to_version
    )))
}

pub(super) fn restore_state(
    paths: &InstallPaths,
    previous: &Option<InstalledState>,
) -> Result<(), String> {
    match previous {
        Some(state) => durable_json(&paths.state, state),
        None if present(&paths.state)? => {
            remove_present(&paths.state, "rolled-back version state")?;
            sync_dir(paths.parent())
        }
        None => Ok(()),
    }
}

pub(super) fn rollback(paths: &InstallPaths) -> Result<(), String> {
    if !present(&paths.backup)? {
        return Err("cannot roll back update: last-known-good binary is missing".into());
    }
    let failed = paths.target.with_extension("jeden-update.failed");
    remove_present(&failed, "earlier failed update")?;
    if present(&paths.target)? {
        fs::rename(&paths.target, &failed)
            .map_err(|error| format!("quarantine failed update: {error}"))?;
    }
    fs::rename(&paths.backup, &paths.target)
        .map_err(|error| format!("restore last-known-good: {error}"))?;
    sync_dir(paths.parent())?;
    remove_present(&failed, "failed update")?;
    Ok(())
}
