//! Writing the updater's own bookkeeping so a power loss cannot leave it
//! half told, and marking the staged binary runnable.
//!
//! Split out of `update/transaction.rs`, which had grown past the module line
//! cap.

use serde::Serialize;
use std::fs::{self, File, OpenOptions};
use std::io::Write;
use std::path::Path;

pub(super) fn sync_dir(path: &Path) -> Result<(), String> {
    File::open(path)
        .and_then(|directory| directory.sync_all())
        .map_err(|error| format!("fsync directory {}: {error}", path.display()))
}

pub(super) fn durable_json<T: Serialize>(path: &Path, value: &T) -> Result<(), String> {
    let temp = path.with_extension("tmp");
    let bytes = serde_json::to_vec(value).map_err(|error| error.to_string())?;
    let mut file = OpenOptions::new()
        .create(true)
        .truncate(true)
        .write(true)
        .open(&temp)
        .map_err(|error| format!("write {}: {error}", temp.display()))?;
    file.write_all(&bytes)
        .and_then(|_| file.write_all(b"\n"))
        .and_then(|_| file.sync_all())
        .map_err(|error| format!("fsync {}: {error}", temp.display()))?;
    fs::rename(&temp, path).map_err(|error| format!("replace {}: {error}", path.display()))?;
    sync_dir(path.parent().ok_or("durable JSON has no parent")?)
}

#[cfg(unix)]
pub(super) fn make_executable(path: &Path) -> Result<(), String> {
    use std::os::unix::fs::PermissionsExt;
    let mut permissions = fs::metadata(path)
        .map_err(|error| error.to_string())?
        .permissions();
    permissions.set_mode(0o755);
    fs::set_permissions(path, permissions).map_err(|error| error.to_string())
}
#[cfg(windows)]
pub(super) fn make_executable(path: &Path) -> Result<(), String> {
    if path
        .extension()
        .and_then(|value| value.to_str())
        .is_some_and(|value| value.eq_ignore_ascii_case("exe"))
    {
        Ok(())
    } else {
        Err("Windows updates require an .exe target".into())
    }
}
#[cfg(not(any(unix, windows)))]
pub(super) fn make_executable(_path: &Path) -> Result<(), String> {
    Err("self-update executable activation is unsupported on this platform".into())
}
