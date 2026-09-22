//! Replacing the running binary as one transaction: stage, back up, rename,
//! and a journal that says which of those had happened if the machine stops.

use serde::{Deserialize, Serialize};
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};

#[derive(Clone, Debug)]
pub struct InstallPaths {
    pub target: PathBuf,
    pub stage: PathBuf,
    pub backup: PathBuf,
    pub journal: PathBuf,
    pub lock: PathBuf,
    pub state: PathBuf,
}

impl InstallPaths {
    pub fn new(target: PathBuf) -> Result<Self, String> {
        let parent = target
            .parent()
            .ok_or("update target has no parent directory")?
            .to_path_buf();
        let name = target
            .file_name()
            .and_then(|name| name.to_str())
            .ok_or("update target name is not UTF-8")?
            .to_owned();
        Ok(Self {
            target,
            stage: parent.join(format!(".{name}.jeden-update.stage")),
            backup: parent.join(format!(".{name}.jeden-update.backup")),
            journal: parent.join(format!(".{name}.jeden-update.journal.json")),
            lock: parent.join(format!(".{name}.jeden-update.lock")),
            state: parent.join(format!(".{name}.jeden-update-state.json")),
        })
    }

    fn parent(&self) -> &Path {
        self.target.parent().expect("validated update target")
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
enum Phase {
    Prepared,
    Staged,
    BackingUp,
    BackedUp,
    Activated,
    Committed,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Journal {
    schema_version: u32,
    phase: Phase,
    from_version: String,
    to_version: String,
    artifact_sha256: String,
    previous_state: Option<InstalledState>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct InstalledState {
    pub schema_version: u32,
    pub version: String,
    pub artifact_sha256: String,
}

mod disk;
mod lock;
mod recover;

use disk::{durable_json, make_executable, sync_dir};
use lock::UpdateLock;
pub use recover::recover;
use recover::{restore_state, rollback};

fn journal(
    paths: &InstallPaths,
    phase: Phase,
    from_version: &str,
    to_version: &str,
    artifact_sha256: &str,
    previous_state: &Option<InstalledState>,
) -> Result<(), String> {
    durable_json(
        &paths.journal,
        &Journal {
            schema_version: 1,
            phase,
            from_version: from_version.into(),
            to_version: to_version.into(),
            artifact_sha256: artifact_sha256.into(),
            previous_state: previous_state.clone(),
        },
    )
}

pub fn recover_exclusive(paths: &InstallPaths) -> Result<Option<String>, String> {
    fs::create_dir_all(paths.parent()).map_err(|error| error.to_string())?;
    let _lock = UpdateLock::acquire(paths)?;
    recover(paths)
}

fn hit(configured: Option<&str>, point: &str) -> Result<(), String> {
    if configured == Some(point) {
        Err(format!("injected updater crash at {point}"))
    } else {
        Ok(())
    }
}


pub fn read_installed_state(paths: &InstallPaths) -> Result<Option<InstalledState>, String> {
    match fs::read(&paths.state) {
        Ok(bytes) => serde_json::from_slice(&bytes)
            .map(Some)
            .map_err(|error| format!("invalid installed update state: {error}")),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(format!("read installed update state: {error}")),
    }
}


pub fn install<F>(
    paths: &InstallPaths,
    artifact: &[u8],
    from_version: &str,
    to_version: &str,
    artifact_sha256: &str,
    failpoint: Option<&str>,
    mut health: F,
) -> Result<(), String>
where
    F: FnMut(&Path, &Path) -> Result<(), String>,
{
    fs::create_dir_all(paths.parent()).map_err(|error| error.to_string())?;
    let _lock = UpdateLock::acquire(paths)?;
    recover(paths)?;
    let reject_staged = |error: String| -> Result<(), String> {
        if paths.stage.exists() {
            fs::remove_file(&paths.stage)
                .map_err(|cleanup| format!("{error}; remove rejected stage: {cleanup}"))?;
        }
        if paths.journal.exists() {
            fs::remove_file(&paths.journal)
                .map_err(|cleanup| format!("{error}; remove rejected journal: {cleanup}"))?;
        }
        sync_dir(paths.parent())?;
        Err(error)
    };
    if paths.stage.exists() || paths.backup.exists() {
        return Err("orphan updater files remain after recovery".into());
    }
    let previous_state = read_installed_state(paths)?;
    journal(
        paths,
        Phase::Prepared,
        from_version,
        to_version,
        artifact_sha256,
        &previous_state,
    )?;
    hit(failpoint, "after-prepared-journal-fsync")?;
    let mut stage = OpenOptions::new()
        .create_new(true)
        .write(true)
        .open(&paths.stage)
        .map_err(|error| format!("stage update: {error}"))?;
    stage
        .write_all(artifact)
        .and_then(|_| stage.sync_all())
        .map_err(|error| format!("fsync staged update: {error}"))?;
    drop(stage);
    make_executable(&paths.stage)?;
    journal(
        paths,
        Phase::Staged,
        from_version,
        to_version,
        artifact_sha256,
        &previous_state,
    )?;
    hit(failpoint, "after-stage-fsync")?;
    if paths.target.exists() {
        if let Err(error) = health(&paths.target, paths.parent()) {
            return reject_staged(format!("pre-update health failed: {error}"));
        }
    }
    if let Err(error) = health(&paths.stage, paths.parent()) {
        return reject_staged(format!("staged update health failed: {error}"));
    }
    journal(
        paths,
        Phase::BackingUp,
        from_version,
        to_version,
        artifact_sha256,
        &previous_state,
    )?;
    if paths.target.exists() {
        fs::rename(&paths.target, &paths.backup)
            .map_err(|error| format!("backup current binary: {error}"))?;
        sync_dir(paths.parent())?;
    }
    hit(failpoint, "after-backup-rename-fsync")?;
    journal(
        paths,
        Phase::BackedUp,
        from_version,
        to_version,
        artifact_sha256,
        &previous_state,
    )?;
    fs::rename(&paths.stage, &paths.target)
        .map_err(|error| format!("activate staged update: {error}"))?;
    sync_dir(paths.parent())?;
    hit(failpoint, "after-activate-rename-fsync")?;
    journal(
        paths,
        Phase::Activated,
        from_version,
        to_version,
        artifact_sha256,
        &previous_state,
    )?;
    if let Err(error) = health(&paths.target, paths.parent()) {
        rollback(paths)?;
        fs::remove_file(&paths.journal).map_err(|remove| {
            format!("{error}; rollback succeeded but journal cleanup failed: {remove}")
        })?;
        sync_dir(paths.parent())?;
        return Err(format!(
            "post-update health failed: {error}; previous binary restored"
        ));
    }
    durable_json(
        &paths.state,
        &InstalledState {
            schema_version: 1,
            version: to_version.into(),
            artifact_sha256: artifact_sha256.into(),
        },
    )?;
    hit(failpoint, "after-state-fsync")?;
    journal(
        paths,
        Phase::Committed,
        from_version,
        to_version,
        artifact_sha256,
        &previous_state,
    )?;
    if paths.backup.exists() {
        fs::remove_file(&paths.backup)
            .map_err(|error| format!("remove last-known-good after commit: {error}"))?;
    }
    fs::remove_file(&paths.journal)
        .map_err(|error| format!("remove committed journal: {error}"))?;
    sync_dir(paths.parent())?;
    Ok(())
}
