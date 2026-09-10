//! Writes into a container file rather than a plain file: one archive entry or
//! one database row. Both halves share the digest reader and the entry-path
//! rule below, and both resolve their target through the write jail, which
//! refuses the workspace's own Jeden state directory.

use sha2::{Digest, Sha256};
use std::fs::File;
use std::io::Read;
use std::path::Path;

mod archive;
mod database;

pub(crate) use archive::write_archive;
pub(crate) use database::write_sqlite;

pub(super) const MAX_ARCHIVE_WRITE_BYTES: u64 = 128 * 1024 * 1024;

/// The digest and byte count of a file, read in bounded chunks so a large
/// archive is never held in memory to be hashed.
pub(super) fn file_sha(path: &Path) -> Result<(String, u64), String> {
    let mut file = File::open(path).map_err(|error| error.to_string())?;
    let mut hash = Sha256::new();
    let mut bytes = 0u64;
    let mut buffer = [0u8; 64 * 1024];
    loop {
        let count = file.read(&mut buffer).map_err(|error| error.to_string())?;
        if count == 0 {
            break;
        }
        hash.update(&buffer[..count]);
        bytes += count as u64;
    }
    Ok((hex::encode(hash.finalize()), bytes))
}

/// An entry name that stays inside the archive it belongs to.
pub(super) fn safe_entry(name: &str) -> Result<(), String> {
    let path = Path::new(name);
    if name.is_empty()
        || path.is_absolute()
        || path
            .components()
            .any(|component| matches!(component, std::path::Component::ParentDir))
    {
        return Err(format!("unsafe archive entry path: {name}"));
    }
    Ok(())
}
