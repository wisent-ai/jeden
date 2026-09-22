//! Applying the operating system's own limits to a child before it becomes
//! the command.
//!
//! Split out of `runtime_ops/host/process.rs`, which had grown past the
//! module line cap.

#[cfg(unix)]
use std::io;
use std::process::Command;

#[cfg(unix)]
pub(super) fn configure_resource_limits(
    command: &mut Command,
    limits: super::ResourceLimits,
) -> Result<(), String> {
    use std::os::unix::process::CommandExt;
    if limits.cpu_seconds == 0
        || limits.address_space_bytes < 16 * 1024 * 1024
        || limits.open_files < 3
        || limits.processes == 0
        || limits.file_bytes == 0
    {
        return Err("invalid zero/unsafe process resource limit".into());
    }
    let inherited_fds = inherited_fds();
    unsafe {
        command.pre_exec(move || {
            mark_inherited_fds_close_on_exec(&inherited_fds);
            set_limit(RLIMIT_CPU, limits.cpu_seconds)?;
            #[cfg(target_os = "linux")]
            set_limit(RLIMIT_AS, limits.address_space_bytes)?;
            set_limit(RLIMIT_NOFILE, limits.open_files)?;
            #[cfg(target_os = "linux")]
            set_limit(RLIMIT_NPROC, limits.processes)?;
            set_limit(RLIMIT_FSIZE, limits.file_bytes)?;
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
#[cfg(target_os = "linux")]
fn inherited_fds() -> Vec<i32> {
    Vec::new()
}
#[cfg(target_os = "macos")]
fn inherited_fds() -> Vec<i32> {
    std::fs::read_dir("/dev/fd")
        .into_iter()
        .flatten()
        .filter_map(|entry| {
            entry
                .ok()?
                .file_name()
                .to_string_lossy()
                .parse::<i32>()
                .ok()
        })
        .filter(|fd| *fd > 2)
        .collect()
}
#[cfg(target_os = "linux")]
fn mark_inherited_fds_close_on_exec(_fds: &[i32]) {
    unsafe {
        syscall(436usize, 3u32, u32::MAX, 4u32);
    }
}
#[cfg(target_os = "macos")]
fn mark_inherited_fds_close_on_exec(fds: &[i32]) {
    for fd in fds {
        unsafe {
            fcntl(*fd, 2, 1);
        }
    }
}
#[cfg(unix)]
fn set_limit(resource: i32, value: u64) -> io::Result<()> {
    let limit = RLimit {
        current: value,
        maximum: value,
    };
    if unsafe { setrlimit(resource, &limit) } == 0 {
        Ok(())
    } else {
        Err(io::Error::last_os_error())
    }
}
#[cfg(unix)]
#[repr(C)]
struct RLimit {
    current: u64,
    maximum: u64,
}
#[cfg(unix)]
extern "C" {
    fn setrlimit(resource: i32, limit: *const RLimit) -> i32;
}
#[cfg(target_os = "linux")]
extern "C" {
    fn syscall(number: usize, ...) -> isize;
}
#[cfg(target_os = "macos")]
extern "C" {
    fn fcntl(fd: i32, command: i32, ...) -> i32;
}
#[cfg(unix)]
const RLIMIT_CPU: i32 = 0;
#[cfg(unix)]
const RLIMIT_FSIZE: i32 = 1;
#[cfg(target_os = "linux")]
const RLIMIT_NPROC: i32 = 6;
#[cfg(target_os = "linux")]
const RLIMIT_NOFILE: i32 = 7;
#[cfg(target_os = "linux")]
const RLIMIT_AS: i32 = 9;
#[cfg(target_os = "macos")]
const RLIMIT_NOFILE: i32 = 8;
