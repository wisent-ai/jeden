use super::security::{ExecutionGrant, GrantError};
#[cfg(unix)]
use std::fs::File;
#[cfg(unix)]
use std::io::{Read, Write};
use std::path::{Component, Path, PathBuf};
pub fn validate_relative(root: &Path, relative: &str) -> Result<PathBuf, GrantError> {
    let path = Path::new(relative.trim());
    if path.is_absolute() {
        return Err(GrantError::FilesystemDenied(
            "absolute path rejected".into(),
        ));
    }
    let mut out = root.to_path_buf();
    for c in path.components() {
        match c {
            Component::Normal(p) => {
                out.push(p);
                if let Ok(m) = std::fs::symlink_metadata(&out) {
                    if m.file_type().is_symlink() {
                        return Err(GrantError::FilesystemDenied(format!(
                            "symbolic link rejected: {}",
                            out.display()
                        )));
                    }
                }
            }
            Component::CurDir => {}
            _ => {
                return Err(GrantError::FilesystemDenied(
                    "unsafe path component rejected".into(),
                ))
            }
        }
    }
    Ok(out)
}
pub struct RootHandle {
    root: PathBuf,
    #[cfg(unix)]
    directory: File,
}
impl RootHandle {
    pub fn open(root: &Path, grant: &ExecutionGrant, write: bool) -> Result<Self, GrantError> {
        if grant.is_expired() {
            return Err(GrantError::Expired);
        }
        let roots = if write {
            &grant.filesystem.write_roots
        } else {
            &grant.filesystem.read_roots
        };
        let canonical = root.canonicalize().map_err(ioerr)?;
        if !roots.iter().any(|v| canonical.starts_with(v)) {
            return Err(GrantError::FilesystemDenied(
                canonical.display().to_string(),
            ));
        }
        #[cfg(unix)]
        {
            let directory = File::open(&canonical).map_err(ioerr)?;
            Ok(Self {
                root: canonical,
                directory,
            })
        }
        #[cfg(not(unix))]
        {
            Err(GrantError::FilesystemDenied(
                "fd-relative filesystem unavailable".into(),
            ))
        }
    }
    pub fn read(&self, relative: &str, maximum: u64) -> Result<Vec<u8>, GrantError> {
        #[cfg(unix)]
        {
            let file = self.openat(relative, false, false)?;
            let size = file.metadata().map_err(ioerr)?.len();
            if size > maximum {
                return Err(GrantError::ResourceLimit(format!(
                    "file size {size} exceeds {maximum}"
                )));
            }
            let mut out = Vec::with_capacity(size as usize);
            file.take(maximum + 1)
                .read_to_end(&mut out)
                .map_err(ioerr)?;
            if out.len() as u64 > maximum {
                return Err(GrantError::ResourceLimit("file grew beyond limit".into()));
            }
            Ok(out)
        }
        #[cfg(not(unix))]
        {
            let _ = (relative, maximum);
            Err(GrantError::FilesystemDenied(
                "fd-relative read unavailable".into(),
            ))
        }
    }
    pub fn create_new(&self, relative: &str, bytes: &[u8], maximum: u64) -> Result<(), GrantError> {
        if bytes.len() as u64 > maximum {
            return Err(GrantError::ResourceLimit("file write exceeds grant".into()));
        }
        #[cfg(unix)]
        {
            let mut file = self.openat(relative, true, true)?;
            file.write_all(bytes)
                .and_then(|_| file.sync_all())
                .map_err(ioerr)
        }
        #[cfg(not(unix))]
        {
            let _ = relative;
            Err(GrantError::FilesystemDenied(
                "fd-relative write unavailable".into(),
            ))
        }
    }
    pub fn root(&self) -> &Path {
        &self.root
    }
    #[cfg(unix)]
    fn openat(&self, r: &str, w: bool, c: bool) -> Result<File, GrantError> {
        unix_openat(&self.directory, r, w, c)
    }
}
fn ioerr(e: std::io::Error) -> GrantError {
    GrantError::FilesystemDenied(e.to_string())
}
#[cfg(unix)]
fn unix_openat(root: &File, relative: &str, write: bool, create: bool) -> Result<File, GrantError> {
    use std::ffi::CString;
    use std::os::fd::{AsRawFd, FromRawFd};
    use std::os::unix::ffi::OsStrExt;
    let path = Path::new(relative);
    if path.as_os_str().is_empty() || path.is_absolute() {
        return Err(GrantError::FilesystemDenied(
            "relative non-empty path required".into(),
        ));
    }
    let parts = path
        .components()
        .map(|c| match c {
            Component::Normal(v) => Ok(v),
            _ => Err(GrantError::FilesystemDenied("unsafe path component".into())),
        })
        .collect::<Result<Vec<_>, _>>()?;
    let mut parent = root.try_clone().map_err(ioerr)?;
    for part in &parts[..parts.len().saturating_sub(1)] {
        let name = CString::new(part.as_bytes())
            .map_err(|_| GrantError::FilesystemDenied("NUL in path".into()))?;
        let fd = unsafe {
            openat(
                parent.as_raw_fd(),
                name.as_ptr(),
                O_RDONLY | O_DIRECTORY | O_NOFOLLOW | O_CLOEXEC,
                0,
            )
        };
        if fd < 0 {
            return Err(ioerr(std::io::Error::last_os_error()));
        }
        parent = unsafe { File::from_raw_fd(fd) }
    }
    let last = parts
        .last()
        .ok_or_else(|| GrantError::FilesystemDenied("empty path".into()))?;
    let name = CString::new(last.as_bytes())
        .map_err(|_| GrantError::FilesystemDenied("NUL in path".into()))?;
    let mut flags = (if write { O_WRONLY } else { O_RDONLY }) | O_NOFOLLOW | O_CLOEXEC;
    if create {
        flags |= O_CREAT | O_EXCL
    }
    let fd = unsafe { openat(parent.as_raw_fd(), name.as_ptr(), flags, 0o600) };
    if fd < 0 {
        return Err(ioerr(std::io::Error::last_os_error()));
    }
    Ok(unsafe { File::from_raw_fd(fd) })
}
#[cfg(unix)]
extern "C" {
    fn openat(dirfd: i32, path: *const i8, flags: i32, ...) -> i32;
}
#[cfg(target_os = "linux")]
const O_RDONLY: i32 = 0;
#[cfg(target_os = "linux")]
const O_WRONLY: i32 = 1;
#[cfg(target_os = "linux")]
const O_CREAT: i32 = 0o100;
#[cfg(target_os = "linux")]
const O_EXCL: i32 = 0o200;
#[cfg(target_os = "linux")]
const O_DIRECTORY: i32 = 0o200000;
#[cfg(target_os = "linux")]
const O_NOFOLLOW: i32 = 0o400000;
#[cfg(target_os = "linux")]
const O_CLOEXEC: i32 = 0o2000000;
#[cfg(target_os = "macos")]
const O_RDONLY: i32 = 0;
#[cfg(target_os = "macos")]
const O_WRONLY: i32 = 1;
#[cfg(target_os = "macos")]
const O_CREAT: i32 = 0x200;
#[cfg(target_os = "macos")]
const O_EXCL: i32 = 0x800;
#[cfg(target_os = "macos")]
const O_DIRECTORY: i32 = 0x100000;
#[cfg(target_os = "macos")]
const O_NOFOLLOW: i32 = 0x100;
#[cfg(target_os = "macos")]
const O_CLOEXEC: i32 = 0x1000000;
