//! Secure temporary files and atomic replacement on unix.
//!
//! Split out of `unix.rs` on the seam between talking to a terminal and
//! writing to a filesystem, so that file fits the three-hundred-line limit
//! the operator's write guard enforces.

use super::super::*;
use super::UnixPlatform;
use std::fs::{self, OpenOptions};
use std::os::unix::fs::OpenOptionsExt;
use std::sync::atomic::{AtomicU64, Ordering};

static TEMP_SEQUENCE: AtomicU64 = AtomicU64::new(1);

impl AtomicFsPlatform for UnixPlatform {
    fn create_secure_temp(
        &self,
        directory: &Path,
        prefix: &OsStr,
    ) -> Result<SecureTemp, PlatformError> {
        fs::create_dir_all(directory)?;
        for _ in 0..128 {
            let n = TEMP_SEQUENCE.fetch_add(1, Ordering::Relaxed);
            let mut name = prefix.to_os_string();
            name.push(format!("-{}-{n}.tmp", std::process::id()));
            let path = directory.join(name);
            match OpenOptions::new()
                .read(true)
                .write(true)
                .create_new(true)
                .mode(0o600)
                .open(&path)
            {
                Ok(file) => return Ok(SecureTemp { path, file }),
                Err(e) if e.kind() == io::ErrorKind::AlreadyExists => continue,
                Err(e) => return Err(e.into()),
            }
        }
        Err(PlatformError::Process(
            "secure temporary-name space exhausted".into(),
        ))
    }
    fn atomic_replace(
        &self,
        staged: &Path,
        destination: &Path,
        backup: Option<&Path>,
    ) -> Result<(), PlatformError> {
        if let Some(backup) = backup {
            if destination.exists() {
                if backup.exists() {
                    fs::remove_file(backup)?;
                }
                fs::hard_link(destination, backup)
                    .or_else(|_| fs::copy(destination, backup).map(|_| ()))?;
                File::open(backup)?.sync_all()?;
            }
        }
        File::open(staged)?.sync_all()?;
        fs::rename(staged, destination)?;
        sync_parent(destination)
    }
}

fn sync_parent(path: &Path) -> Result<(), PlatformError> {
    let parent = path.parent().ok_or_else(|| {
        PlatformError::Process("atomic replacement has no parent directory".into())
    })?;
    File::open(parent)?.sync_all()?;
    Ok(())
}
