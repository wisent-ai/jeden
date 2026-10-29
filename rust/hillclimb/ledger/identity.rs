//! Stable default-ledger identities, without replacing native path units.
use std::path::{Path, PathBuf};

use sha2::{Digest, Sha256};

pub(super) fn path(configuration_directory: &Path, canonical: &Path) -> Result<PathBuf, String> {
    let directory = configuration_directory.join("hillclimb");
    let path = directory.join(format!("{}.jsonl", key(canonical)?));
    if canonical.to_str().is_some() || entry_exists(&path)? {
        return Ok(path);
    }
    // Only migration diagnosis uses the former lossy representation. Never
    // use it to choose or adopt a workspace's current history or writer lock.
    let legacy = hex::encode(Sha256::digest(canonical.to_string_lossy().as_bytes()));
    let legacy_path = directory.join(format!("{legacy}.jsonl"));
    let legacy_lock = legacy_path.with_extension("jsonl.lock");
    let legacy_entry = if entry_exists(&legacy_path)? {
        Some(&legacy_path)
    } else if entry_exists(&legacy_lock)? {
        Some(&legacy_lock)
    } else {
        None
    };
    if let Some(entry) = legacy_entry {
        return Err(format!(
            "the canonical hill-climb scope {canonical:?} has native path units that are not Unicode, and legacy identity entry {entry:?} exists under its lossy key; that entry cannot identify the history's scope unambiguously and a lock sidecar alone does not prove a live worker. Nothing was adopted or rewritten. Select the intended history explicitly with --ledger {legacy_path:?}, or explicitly select the new lossless ledger {path:?}"
        ));
    }
    Ok(path)
}

fn entry_exists(path: &Path) -> Result<bool, String> {
    match std::fs::symlink_metadata(path) {
        Ok(_) => Ok(true),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(error) => Err(format!("the hill-climb ledger identity entry {path:?} cannot be inspected: {error}")),
    }
}

fn key(canonical: &Path) -> Result<String, String> {
    if let Some(unicode) = canonical.to_str() {
        // Preserve every existing Unicode workspace's UTF-8 ledger identity.
        return Ok(hex::encode(Sha256::digest(unicode.as_bytes())));
    }
    #[cfg(unix)]
    {
        use std::os::unix::ffi::OsStrExt;
        Ok(hex::encode(Sha256::digest(canonical.as_os_str().as_bytes())))
    }
    #[cfg(windows)]
    {
        use std::os::windows::ffi::OsStrExt;
        let mut digest = Sha256::new();
        // A NUL-containing domain cannot be an ordinary Unicode path. Native
        // UTF-16 units use explicit byte order, not an unspecified OsStr encoding.
        digest.update(b"windows-wide\0");
        for unit in canonical.as_os_str().encode_wide() {
            digest.update(unit.to_le_bytes());
        }
        Ok(hex::encode(digest.finalize()))
    }
    #[cfg(not(any(unix, windows)))]
    {
        Err(format!("the hill-climb scope {canonical:?} is not Unicode and this platform has no declared stable native-path encoding"))
    }
}
