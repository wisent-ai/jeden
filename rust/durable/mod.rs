//! Replacing a file's whole content so a reader sees the previous document
//! or the new one, never a torn write. The bytes are staged in a private
//! file beside the destination, made durable, and renamed over it through
//! the platform's own secure-temp and atomic-replace operations, so the
//! staged name owes nothing to the clock and nothing to a second copy of
//! those steps.

use std::ffi::OsString;
use std::io::Write;
use std::path::Path;

use super::platform::{native, PlatformError, SecureTemp};

/// Write `bytes` as the whole content of `path`. When a step fails the
/// staged file is removed; a staged file that cannot be removed is named
/// in the error beside the cause.
pub fn replace(path: &Path, bytes: &[u8]) -> Result<(), String> {
    let parent = path
        .parent()
        .ok_or_else(|| format!("{} has no parent directory", path.display()))?;
    let name = path
        .file_name()
        .ok_or_else(|| format!("{} names no file to replace", path.display()))?;
    let mut prefix = OsString::from(".");
    prefix.push(name);
    let platform = native();
    let SecureTemp {
        path: staging,
        mut file,
    } = platform
        .create_secure_temp(parent, &prefix)
        .map_err(|error| format!("cannot stage {}: {error}", path.display()))?;
    let written = file.write_all(bytes).and_then(|()| file.flush());
    drop(file);
    let replaced = match written {
        Ok(()) => platform.atomic_replace(&staging, path, None),
        Err(error) => Err(PlatformError::from(error)),
    };
    let Err(error) = replaced else {
        return Ok(());
    };
    let refused = format!("cannot persist {}: {error}", path.display());
    match std::fs::remove_file(&staging) {
        Ok(()) => Err(refused),
        Err(left) if left.kind() == std::io::ErrorKind::NotFound => Err(refused),
        Err(left) => Err(format!(
            "{refused}; {} is left behind: {left}",
            staging.display()
        )),
    }
}
