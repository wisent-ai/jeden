//! Applying the operating system's own limits to a child before it becomes
//! the command, through libc's declarations of setrlimit, the RLIMIT_*
//! resources, close_range and F_SETFD. They were written here by hand as
//! numbers and their results were dropped: an inherited descriptor that could
//! not be marked close-on-exec stayed open in the command without a word.
//!
//! Split out of `runtime_ops/host/process.rs`, which had grown past the
//! module line cap.

#[cfg(unix)]
use std::io;
use std::process::Command;

/// The type setrlimit names its resource with: glibc declares an enum,
/// every other libc a plain int.
#[cfg(all(target_os = "linux", target_env = "gnu"))]
type Resource = libc::__rlimit_resource_t;
#[cfg(all(unix, not(all(target_os = "linux", target_env = "gnu"))))]
type Resource = libc::c_int;

/// The standard input, output and error every command keeps open; they are
/// the lowest descriptors, so the first inherited one is numbered by their
/// count.
#[cfg(unix)]
const STANDARD: &[libc::c_int] = &[libc::STDIN_FILENO, libc::STDOUT_FILENO, libc::STDERR_FILENO];

#[cfg(unix)]
pub(super) fn configure_resource_limits(
    command: &mut Command,
    limits: super::ResourceLimits,
) -> Result<(), String> {
    use std::num::NonZeroU64;
    use std::os::unix::process::CommandExt;
    let named = [
        ("cpuSeconds", limits.cpu_seconds),
        ("addressSpaceBytes", limits.address_space_bytes),
        ("openFiles", limits.open_files),
        ("processes", limits.processes),
        ("fileBytes", limits.file_bytes),
    ];
    if let Some((name, _zero)) = named.iter().find(|(_, value)| NonZeroU64::new(*value).is_none()) {
        return Err(format!(
            "process resource limit {name} is 0, which would let the command run nothing"
        ));
    }
    let standard = u64::try_from(STANDARD.len())
        .map_err(|error| format!("the standard descriptors cannot be counted: {error}"))?;
    if limits.open_files < standard {
        return Err(format!(
            "process resource limit openFiles is {}, which leaves the command no room for its standard input, output and error",
            limits.open_files
        ));
    }
    let inherited_fds = inherited_fds()?;
    unsafe {
        command.pre_exec(move || {
            mark_inherited_fds_close_on_exec(&inherited_fds)?;
            set_limit(libc::RLIMIT_CPU, limits.cpu_seconds)?;
            #[cfg(target_os = "linux")]
            set_limit(libc::RLIMIT_AS, limits.address_space_bytes)?;
            set_limit(libc::RLIMIT_NOFILE, limits.open_files)?;
            #[cfg(target_os = "linux")]
            set_limit(libc::RLIMIT_NPROC, limits.processes)?;
            set_limit(libc::RLIMIT_FSIZE, limits.file_bytes)?;
            Ok(())
        });
    }
    Ok(())
}
#[cfg(not(unix))]
pub(super) fn configure_resource_limits(
    _command: &mut Command,
    _limits: super::ResourceLimits,
) -> Result<(), String> {
    Err("native resource-limit backend unavailable".into())
}

/// Linux closes the inherited descriptors in the child by range, so none
/// need listing.
#[cfg(target_os = "linux")]
fn inherited_fds() -> Result<Vec<libc::c_int>, String> {
    Ok(Vec::new())
}
/// The descriptors this process holds above standard error, as /dev/fd lists
/// them; a listing or an entry that cannot be read is refused naming it.
#[cfg(target_os = "macos")]
fn inherited_fds() -> Result<Vec<libc::c_int>, String> {
    let entries = std::fs::read_dir("/dev/fd")
        .map_err(|error| format!("/dev/fd cannot be listed to close inherited descriptors: {error}"))?;
    let mut fds = Vec::new();
    for entry in entries {
        let entry = entry.map_err(|error| format!("an entry of /dev/fd cannot be read: {error}"))?;
        let name = entry.file_name();
        let text = name
            .to_str()
            .ok_or_else(|| format!("/dev/fd lists {name:?}, which is not text"))?;
        let fd = text
            .parse::<libc::c_int>()
            .map_err(|error| format!("/dev/fd lists {text:?}, which is no descriptor: {error}"))?;
        if !STANDARD.contains(&fd) {
            fds.push(fd);
        }
    }
    Ok(fds)
}

/// Mark every descriptor above the standard ones close-on-exec in the child.
#[cfg(target_os = "linux")]
fn mark_inherited_fds_close_on_exec(_fds: &[libc::c_int]) -> io::Result<()> {
    let first = libc::c_uint::try_from(STANDARD.len()).map_err(io::Error::other)?;
    let rc = unsafe {
        libc::syscall(libc::SYS_close_range, first, libc::c_uint::MAX, libc::CLOSE_RANGE_CLOEXEC)
    };
    if rc.is_negative() {
        return Err(io::Error::last_os_error());
    }
    Ok(())
}
#[cfg(target_os = "macos")]
fn mark_inherited_fds_close_on_exec(fds: &[libc::c_int]) -> io::Result<()> {
    for fd in fds {
        let rc = unsafe { libc::fcntl(*fd, libc::F_SETFD, libc::FD_CLOEXEC) };
        // A descriptor already closed (the /dev/fd listing's own) has
        // nothing left to keep from the command.
        if rc.is_negative() {
            let error = io::Error::last_os_error();
            if error.raw_os_error() != Some(libc::EBADF) {
                return Err(error);
            }
        }
    }
    Ok(())
}

#[cfg(unix)]
fn set_limit(resource: Resource, value: u64) -> io::Result<()> {
    let limit = libc::rlimit {
        rlim_cur: value,
        rlim_max: value,
    };
    if unsafe { libc::setrlimit(resource, &limit) }.is_negative() {
        Err(io::Error::last_os_error())
    } else {
        Ok(())
    }
}
