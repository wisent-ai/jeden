//! The private gateway's filesystem ownership, held until its child is reaped.
use super::directory;

/// The ownership lease of the private gateway, held for the child's life and
/// released when its File drops with the gateway it admits.
pub(super) fn lease() -> Result<std::fs::File, String> {
    use std::os::unix::io::AsRawFd;
    private_directory(&directory())?;
    let path = directory().join("gateway.lock");
    let file = std::fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .open(&path)
        .map_err(|error| {
            format!(
                "{} cannot be opened for Jeden's own gateway: {error}",
                path.display()
            )
        })?;
    // SAFETY: flock on a descriptor this process owns; the exclusive lock
    // is let go when the File drops with the gateway it admits.
    let locked = unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) };
    if locked == libc::EXIT_SUCCESS {
        return Ok(file);
    }
    let error = std::io::Error::last_os_error();
    if error.raw_os_error() == Some(libc::EWOULDBLOCK) {
        Err(format!(
            "another Jeden process on this machine runs its own gateway and owns the subscriptions' renewals ({} is held); sign in from it or close it first",
            path.display()
        ))
    } else {
        Err(format!(
            "{} cannot be locked for Jeden's own gateway: {error}",
            path.display()
        ))
    }
}

/// Create `dir` for this user alone, as the token store's directory is.
pub(super) fn private_directory(dir: &std::path::Path) -> Result<(), String> {
    use std::os::unix::fs::DirBuilderExt;
    std::fs::DirBuilder::new()
        .recursive(true)
        .mode(crate::tool_runtime::runtime_ops::platform::mode::OWNER_ALL)
        .create(dir)
        .map_err(|error| {
            format!(
                "cannot create Jeden's gateway directory {}: {error}",
                dir.display()
            )
        })
}

#[cfg(test)]
#[path = "../../../../tests/model/credentials/gateway_lifecycle.rs"]
mod lifecycle;
