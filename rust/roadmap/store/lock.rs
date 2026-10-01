//! One writer at a time on the roadmap file.
//!
//! Split out of `roadmap/mod.rs`, which had grown past the module line cap.
//!
//! The lock is the kernel's advisory lock on the lock file: a second writer
//! blocks until the first one's file closes, which also happens when its
//! process dies, so there is no retry count, no pause between tries and no
//! stale lock file to wait out.

use super::super::model::RoadmapError;
use std::fs::{self, File, OpenOptions};
use std::path::Path;

pub(super) struct StableLock {
    _file: File,
}

impl StableLock {
    pub(super) fn acquire(path: &Path) -> Result<Self, RoadmapError> {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        let file = OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(false)
            .open(path)?;
        file.lock().map_err(|error| {
            RoadmapError::Io(format!("roadmap lock {} refused: {error}", path.display()))
        })?;
        Ok(Self { _file: file })
    }
}
