//! Writing the updater's own bookkeeping so a power loss cannot leave it
//! half told, and marking the staged binary runnable.
//!
//! Split out of `update/transaction.rs`, which had grown past the module line
//! cap.

use serde::Serialize;
use std::fs::{self, File};
use std::path::Path;

pub(super) fn sync_dir(path: &Path) -> Result<(), String> {
    File::open(path)
        .and_then(|directory| directory.sync_all())
        .map_err(|error| format!("fsync directory {}: {error}", path.display()))
}

/// `value` as JSON over `path` through the shared durable replace. The
/// staging file was one fixed <name>.tmp, so two updaters wrote into one.
pub(super) fn durable_json<T: Serialize>(path: &Path, value: &T) -> Result<(), String> {
    let mut bytes =
        serde_json::to_vec(value).map_err(|error| format!("encode {}: {error}", path.display()))?;
    bytes.push(b'\n');
    crate::tool_runtime::runtime_ops::durable::replace(path, &bytes)
}

#[cfg(unix)]
pub(super) fn make_executable(path: &Path) -> Result<(), String> {
    use std::os::unix::fs::PermissionsExt;
    let mut permissions = fs::metadata(path)
        .map_err(|error| format!("read the permissions of {}: {error}", path.display()))?
        .permissions();
    // Owner reads, writes and runs it; group and others read and run it.
    permissions.set_mode(u32::from(
        libc::S_IRWXU | libc::S_IRGRP | libc::S_IXGRP | libc::S_IROTH | libc::S_IXOTH,
    ));
    fs::set_permissions(path, permissions)
        .map_err(|error| format!("mark {} executable: {error}", path.display()))
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
