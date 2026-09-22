//! Finishing or undoing an update that was interrupted, using the journal the
//! installer wrote before each step.
//!
//! Split out of `update/transaction.rs`, which had grown past the module line
//! cap.

use super::disk::{durable_json, sync_dir};
use super::{InstallPaths, InstalledState, Journal, Phase};
use std::fs;

pub fn recover(paths: &InstallPaths) -> Result<Option<String>, String> {
    let bytes = match fs::read(&paths.journal) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(format!("read update journal: {error}")),
    };
    let entry: Journal = serde_json::from_slice(&bytes)
        .map_err(|error| format!("invalid update journal: {error}"))?;
    if entry.schema_version != 1 {
        return Err(format!(
            "unsupported update journal schema {}",
            entry.schema_version
        ));
    }
    match entry.phase {
        Phase::Prepared | Phase::Staged => {
            if paths.stage.exists() {
                fs::remove_file(&paths.stage)
                    .map_err(|error| format!("remove interrupted stage: {error}"))?;
            }
        }
        Phase::BackingUp | Phase::BackedUp => {
            if paths.backup.exists() {
                if paths.target.exists() {
                    fs::remove_file(&paths.target)
                        .map_err(|error| format!("remove uncertain target: {error}"))?;
                }
                fs::rename(&paths.backup, &paths.target)
                    .map_err(|error| format!("restore last-known-good: {error}"))?;
            }
            if paths.stage.exists() {
                fs::remove_file(&paths.stage)
                    .map_err(|error| format!("remove interrupted stage: {error}"))?;
            }
        }
        Phase::Activated => {
            rollback(paths)?;
            restore_state(paths, &entry.previous_state)?;
        }
        Phase::Committed => {
            if paths.backup.exists() {
                fs::remove_file(&paths.backup)
                    .map_err(|error| format!("remove committed backup: {error}"))?;
            }
            if paths.stage.exists() {
                fs::remove_file(&paths.stage)
                    .map_err(|error| format!("remove committed stage: {error}"))?;
            }
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

pub(super) fn restore_state(paths: &InstallPaths, previous: &Option<InstalledState>) -> Result<(), String> {
    match previous {
        Some(state) => durable_json(&paths.state, state),
        None if paths.state.exists() => {
            fs::remove_file(&paths.state)
                .map_err(|error| format!("remove rolled-back version state: {error}"))?;
            sync_dir(paths.parent())
        }
        None => Ok(()),
    }
}

pub(super) fn rollback(paths: &InstallPaths) -> Result<(), String> {
    if !paths.backup.exists() {
        return Err("cannot roll back update: last-known-good binary is missing".into());
    }
    let failed = paths.target.with_extension("jeden-update.failed");
    if failed.exists() {
        fs::remove_file(&failed).map_err(|error| error.to_string())?;
    }
    if paths.target.exists() {
        fs::rename(&paths.target, &failed)
            .map_err(|error| format!("quarantine failed update: {error}"))?;
    }
    fs::rename(&paths.backup, &paths.target)
        .map_err(|error| format!("restore last-known-good: {error}"))?;
    sync_dir(paths.parent())?;
    if failed.exists() {
        fs::remove_file(failed).map_err(|error| format!("remove failed update: {error}"))?;
    }
    Ok(())
}
