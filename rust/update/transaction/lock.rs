//! One updater at a time on this machine, and a way out when the previous
//! one died.
//!
//! Split out of `update/transaction.rs`, which had grown past the module line
//! cap.

use super::{sync_dir, InstallPaths};
use std::fs::{self, File, OpenOptions};
use std::io::Write;
use std::path::PathBuf;

pub(super) struct UpdateLock {
    path: PathBuf,
    parent: PathBuf,
    _file: File,
}

impl UpdateLock {
    pub(super) fn acquire(paths: &InstallPaths) -> Result<Self, String> {
        let open = || {
            OpenOptions::new()
                .create_new(true)
                .write(true)
                .open(&paths.lock)
        };
        let mut file = match open() {
            Ok(file) => file,
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
                let owner = match fs::read_to_string(&paths.lock) {
                    Ok(value) => value.trim().parse::<u32>().ok(),
                    Err(error) => {
                        return Err(format!(
                            "cannot read the update lock {} to see which updater holds it: {error}",
                            paths.lock.display()
                        ))
                    }
                };
                if owner.is_none() || owner.is_some_and(process_alive) {
                    return Err("another updater holds the durable update lock".into());
                }
                fs::remove_file(&paths.lock)
                    .map_err(|remove| format!("remove stale update lock: {remove}"))?;
                open().map_err(|retry| {
                    if retry.kind() == std::io::ErrorKind::AlreadyExists {
                        "another updater acquired the durable update lock".into()
                    } else {
                        format!("acquire update lock: {retry}")
                    }
                })?
            }
            Err(error) => return Err(format!("acquire update lock: {error}")),
        };
        writeln!(file, "{}", std::process::id())
            .and_then(|_| file.sync_all())
            .map_err(|error| format!("persist update lock owner: {error}"))?;
        sync_dir(paths.parent())?;
        Ok(Self {
            path: paths.lock.clone(),
            parent: paths.parent().to_path_buf(),
            _file: file,
        })
    }
}

#[cfg(unix)]
fn process_alive(pid: u32) -> bool {
    unsafe extern "C" {
        fn kill(pid: i32, signal: i32) -> i32;
    }
    unsafe { kill(pid as i32, 0) == 0 }
}

#[cfg(windows)]
fn process_alive(pid: u32) -> bool {
    unsafe extern "system" {
        fn OpenProcess(access: u32, inherit: i32, pid: u32) -> *mut std::ffi::c_void;
        fn CloseHandle(handle: *mut std::ffi::c_void) -> i32;
    }
    const PROCESS_QUERY_LIMITED_INFORMATION: u32 = 0x1000;
    let handle = unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid) };
    if handle.is_null() {
        false
    } else {
        unsafe {
            CloseHandle(handle);
        }
        true
    }
}

#[cfg(not(any(unix, windows)))]
fn process_alive(_pid: u32) -> bool {
    true
}

impl Drop for UpdateLock {
    // A lock that stays is taken back only once its process is gone; until
    // then, and whenever the release fails, that is said with path and cause.
    fn drop(&mut self) {
        match fs::remove_file(&self.path) {
            Ok(()) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => eprintln!(
                "jeden update: the update lock {} is left behind: {error}",
                self.path.display()
            ),
        }
        if let Err(error) = sync_dir(&self.parent) {
            eprintln!("jeden update: the release of the update lock was not synced: {error}");
        }
    }
}
