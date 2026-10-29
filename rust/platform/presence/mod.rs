//! Whether a path is there, told apart from whether it can be examined.
//!
//! `Path::exists` answers false for a path it cannot stat (a permission or
//! I/O fault), so a session, a replay stream or a recorder state read
//! through it was taken as absent: reported as not found, started empty, or
//! written over. Only a path that does not exist is absent here; every other
//! cause is the error, naming the path.

use std::path::Path;

/// True when `path` exists, false when it does not; a path that cannot be
/// examined is an error naming the path and the cause.
pub fn present(path: &Path) -> Result<bool, String> {
    match std::fs::metadata(path) {
        Ok(_) => Ok(true),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(error) => Err(format!("{} cannot be examined: {error}", path.display())),
    }
}

/// Remove the file `path` when it is there; `what` names it in a refusal.
/// Only a file already gone counts as removed; every other failure is
/// refused naming the path and the cause.
pub fn remove_present(path: &Path, what: &str) -> Result<(), String> {
    match std::fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(format!(
            "{what} {} cannot be removed: {error}",
            path.display()
        )),
    }
}
