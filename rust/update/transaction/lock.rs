//! One updater at a time on this machine. The lock is the operating system's
//! exclusive lock on one file beside the install, so it is released when its
//! holder exits however it exits: a dead updater leaves no lock to take back.
//! The file stays in place between updates; it records the process that last
//! held it, which a refused updater names.
//!
//! Split out of `update/transaction.rs`, which had grown past the module line
//! cap.

use super::{sync_dir, InstallPaths};
use std::fs::{self, File, OpenOptions, TryLockError};
use std::io::{Seek, Write};

pub(super) struct UpdateLock {
    _file: File,
}

impl UpdateLock {
    pub(super) fn acquire(paths: &InstallPaths) -> Result<Self, String> {
        let mut file = OpenOptions::new()
            .create(true)
            .truncate(false)
            .write(true)
            .open(&paths.lock)
            .map_err(|error| {
                format!(
                    "cannot open the update lock {}: {error}",
                    paths.lock.display()
                )
            })?;
        match file.try_lock() {
            Ok(()) => {}
            Err(TryLockError::WouldBlock) => {
                return Err(match fs::read_to_string(&paths.lock) {
                    Ok(holder) => format!(
                        "another updater holds the update lock {} (process {})",
                        paths.lock.display(),
                        holder.trim()
                    ),
                    Err(error) => format!(
                        "another updater holds the update lock {}, and which one cannot be read: {error}",
                        paths.lock.display()
                    ),
                })
            }
            Err(TryLockError::Error(error)) => {
                return Err(format!("cannot lock {}: {error}", paths.lock.display()))
            }
        }
        // The holder replaces whatever an earlier holder recorded.
        let holder = format!("{}\n", std::process::id());
        file.rewind()
            .and_then(|()| file.write_all(holder.as_bytes()))
            .and_then(|()| file.set_len(holder.len() as u64))
            .and_then(|()| file.sync_all())
            .map_err(|error| format!("persist update lock owner: {error}"))?;
        sync_dir(paths.parent())?;
        Ok(Self { _file: file })
    }
}
