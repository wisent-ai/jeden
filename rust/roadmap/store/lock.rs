//! One writer at a time on the roadmap file.
//!
//! Split out of `roadmap/mod.rs`, which had grown past the module line cap.

use super::super::model::RoadmapError;
use std::fs::{self, File, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::thread;
use std::time::Duration;

const LOCK_RETRIES: usize = 500;
const LOCK_WAIT: Duration = Duration::from_millis(10);

pub(super) struct StableLock {
    path: PathBuf,
    _file: File,
}

impl Drop for StableLock {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.path);
    }
}

impl StableLock {
    pub(super) fn acquire(path: &Path) -> Result<Self, RoadmapError> {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        for _ in 0..LOCK_RETRIES {
            match OpenOptions::new().write(true).create_new(true).open(path) {
                Ok(mut file) => {
                    writeln!(file, "{}", std::process::id())?;
                    file.sync_all()?;
                    return Ok(Self {
                        path: path.to_path_buf(),
                        _file: file,
                    });
                }
                Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
                    thread::sleep(LOCK_WAIT);
                }
                Err(error) => return Err(error.into()),
            }
        }
        Err(RoadmapError::Io(format!(
            "timed out waiting for roadmap lock {}",
            path.display()
        )))
    }
}
