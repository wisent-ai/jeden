//! Secure temporary files, owner-only ACLs, and atomic replacement on Windows.
//!
//! Split out of `windows.rs` on the seam between driving a console and writing
//! to a filesystem, so that file fits the three-hundred-line limit the
//! operator's write guard enforces. The Win32 declarations it calls stay in
//! the parent module, so nothing here required a visibility change.

use super::*;
use std::ffi::OsStr;
use std::fs::{self, OpenOptions};
use std::path::Path;
use std::ptr::{null, null_mut};
use std::sync::atomic::Ordering;

impl AtomicFsPlatform for NativePlatform {
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
                .open(&path)
            {
                Ok(file) => {
                    secure_acl(&path)?;
                    return Ok(SecureTemp { path, file });
                }
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
        let source = wide(staged.as_os_str());
        let dest = wide(destination.as_os_str());
        if destination.exists() {
            let backup_w = backup.map(|p| wide(p.as_os_str()));
            if unsafe {
                ReplaceFileW(
                    dest.as_ptr(),
                    source.as_ptr(),
                    backup_w.as_ref().map_or(null(), |v| v.as_ptr()),
                    REPLACEFILE_WRITE_THROUGH,
                    null_mut(),
                    null_mut(),
                )
            } == 0
            {
                return Err(last_error());
            }
        } else if unsafe {
            MoveFileExW(
                source.as_ptr(),
                dest.as_ptr(),
                MOVEFILE_REPLACE_EXISTING | MOVEFILE_WRITE_THROUGH,
            )
        } == 0
        {
            return Err(last_error());
        }
        Ok(())
    }
}

fn secure_acl(path: &Path) -> Result<(), PlatformError> {
    unsafe {
        let sddl = wide(OsStr::new("D:P(A;;FA;;;OW)"));
        let mut sd = null_mut();
        if ConvertStringSecurityDescriptorToSecurityDescriptorW(
            sddl.as_ptr(),
            1,
            &mut sd,
            null_mut(),
        ) == 0
        {
            return Err(last_error());
        }
        let (mut present, mut defaulted, mut acl) = (0, 0, null_mut());
        if GetSecurityDescriptorDacl(sd, &mut present, &mut acl, &mut defaulted) == 0 {
            LocalFree(sd);
            return Err(last_error());
        }
        let p = wide(path.as_os_str());
        let rc = SetNamedSecurityInfoW(
            p.as_ptr() as *mut _,
            1,
            0x8000_0004,
            null_mut(),
            null_mut(),
            acl,
            null_mut(),
        );
        LocalFree(sd);
        if rc != 0 {
            return Err(io::Error::from_raw_os_error(rc as i32).into());
        }
        Ok(())
    }
}
