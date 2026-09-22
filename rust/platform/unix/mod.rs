use super::*;
use std::fs::File;
use std::io::{Read, Write};
use std::os::fd::{FromRawFd, RawFd};
use std::os::unix::process::CommandExt;
use std::process::{Command, Stdio};

mod atomicfs;
mod workspace;

pub(crate) struct UnixPlatform;
impl UnixPlatform {
    pub const fn new() -> Self {
        Self
    }
}

struct UnixProcessTree {
    group: i32,
}
impl ProcessTree for UnixProcessTree {
    fn signal(&mut self, signal: ProcessSignal) -> Result<(), PlatformError> {
        let number = match signal {
            ProcessSignal::Interrupt => 2,
            ProcessSignal::Terminate => 15,
            ProcessSignal::Kill => 9,
        };
        let rc = unsafe { kill(-self.group, number) };
        if rc < 0 {
            let error = io::Error::last_os_error();
            if error.raw_os_error() != Some(3) {
                return Err(error.into());
            }
        }
        Ok(())
    }
}

impl ProcessPlatform for UnixPlatform {
    fn configure_command(&self, command: &mut Command) -> Result<(), PlatformError> {
        unsafe {
            command.pre_exec(|| {
                if setpgid(0, 0) == 0 {
                    Ok(())
                } else {
                    Err(io::Error::last_os_error())
                }
            });
        }
        Ok(())
    }
    fn attach_process_tree(&self, child: &Child) -> Result<Box<dyn ProcessTree>, PlatformError> {
        Ok(Box::new(UnixProcessTree {
            group: child.id() as i32,
        }))
    }
    fn pipe_reader(
        &self,
        pipe: Box<dyn Read + Send>,
    ) -> Result<Box<dyn PipeReader>, PlatformError> {
        Ok(threaded_pipe(pipe))
    }
}

struct UnixPtySession {
    child: Child,
    master: File,
    slave: File,
    group: i32,
}
impl PtySession for UnixPtySession {
    fn process_id(&self) -> u32 {
        self.child.id()
    }
    fn write_all(&mut self, bytes: &[u8]) -> io::Result<()> {
        self.master
            .write_all(bytes)
            .and_then(|_| self.master.flush())
    }
    fn read_available(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
        self.master.read(buffer)
    }
    fn resize(&mut self, cols: u16, rows: u16) -> Result<(), PlatformError> {
        set_window_size(self.master_fd(), cols, rows)?;
        set_window_size(self.slave_fd(), cols, rows)?;
        let rc = unsafe { kill(-self.group, SIGWINCH) };
        if rc < 0 {
            return Err(io::Error::last_os_error().into());
        }
        Ok(())
    }
    fn alive(&mut self) -> Result<bool, PlatformError> {
        Ok(self.child.try_wait()?.is_none())
    }
    fn exit_status(&mut self) -> Result<Option<ExitStatus>, PlatformError> {
        Ok(self.child.try_wait()?)
    }
    fn signal(&mut self, signal: ProcessSignal) -> Result<(), PlatformError> {
        let number = match signal {
            ProcessSignal::Interrupt => 2,
            ProcessSignal::Terminate => 15,
            ProcessSignal::Kill => 9,
        };
        let rc = unsafe { kill(-self.group, number) };
        if rc < 0 {
            let error = io::Error::last_os_error();
            if error.raw_os_error() != Some(3) {
                return Err(error.into());
            }
        }
        Ok(())
    }
}
impl UnixPtySession {
    fn master_fd(&self) -> RawFd {
        use std::os::fd::AsRawFd;
        self.master.as_raw_fd()
    }
    fn slave_fd(&self) -> RawFd {
        use std::os::fd::AsRawFd;
        self.slave.as_raw_fd()
    }
}
impl Drop for UnixPtySession {
    fn drop(&mut self) {
        let _ = self.signal(ProcessSignal::Kill);
        let _ = self.child.wait();
    }
}

impl PtyPlatform for UnixPlatform {
    fn spawn_shell(
        &self,
        cwd: &Path,
        cols: u16,
        rows: u16,
    ) -> Result<Box<dyn PtySession>, PlatformError> {
        let mut master = -1;
        let mut slave = -1;
        if unsafe {
            openpty(
                &mut master,
                &mut slave,
                std::ptr::null_mut(),
                std::ptr::null(),
                std::ptr::null(),
            )
        } < 0
        {
            return Err(io::Error::last_os_error().into());
        }
        if let Err(error) =
            set_window_size(master, cols, rows).and_then(|_| set_window_size(slave, cols, rows))
        {
            unsafe {
                close(master);
                close(slave);
            }
            return Err(error);
        }
        let stdin_fd = duplicate(slave)?;
        let stdout_fd = duplicate(slave)?;
        let stderr_fd = duplicate(slave)?;
        let mut command = Command::new("/bin/sh");
        command.arg("-i").current_dir(cwd);
        unsafe {
            command
                .stdin(Stdio::from_raw_fd(stdin_fd))
                .stdout(Stdio::from_raw_fd(stdout_fd))
                .stderr(Stdio::from_raw_fd(stderr_fd));
            command.pre_exec(|| {
                if setsid() < 0 {
                    Err(io::Error::last_os_error())
                } else {
                    Ok(())
                }
            });
        }
        let child = command.spawn()?;
        let group = child.id() as i32;
        let master_file = unsafe { File::from_raw_fd(master) };
        let slave_file = unsafe { File::from_raw_fd(slave) };
        set_nonblocking(master)?;
        Ok(Box::new(UnixPtySession {
            child,
            master: master_file,
            slave: slave_file,
            group,
        }))
    }
    fn startup_handshake(&self) -> (&'static [u8], &'static [u8]) {
        (
            b"stty -echo; printf '\n__JEDEN_%s_READY__\n' PTY\n",
            b"__JEDEN_PTY_READY__",
        )
    }
    fn command_frame(&self, input: &str, process_id: u32, sequence: u64) -> PtyCommandFrame {
        let marker = format!("__JEDEN_PTY_{process_id}_{sequence}__");
        let bytes = format!("{input}\ns=$?; printf '\\n{marker}:%s\\n' \"$s\"\n").into_bytes();
        PtyCommandFrame { marker, bytes }
    }
}
impl DesktopPlatform for UnixPlatform {
    fn open_path(&self, path: &Path) -> Result<(), PlatformError> {
        #[cfg(target_os = "macos")]
        let program = "open";
        #[cfg(target_os = "linux")]
        let program = "xdg-open";
        Command::new(program)
            .arg(path)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()?;
        Ok(())
    }
}

fn duplicate(fd: RawFd) -> Result<RawFd, PlatformError> {
    let value = unsafe { dup(fd) };
    if value < 0 {
        Err(io::Error::last_os_error().into())
    } else {
        Ok(value)
    }
}
fn set_nonblocking(fd: RawFd) -> Result<(), PlatformError> {
    let flags = unsafe { fcntl(fd, 3, 0) };
    if flags < 0 {
        return Err(io::Error::last_os_error().into());
    }
    if unsafe { fcntl(fd, 4, flags | 4) } < 0 {
        return Err(io::Error::last_os_error().into());
    }
    Ok(())
}
fn set_window_size(fd: RawFd, cols: u16, rows: u16) -> Result<(), PlatformError> {
    let size = Winsize {
        rows,
        cols,
        xpixel: 0,
        ypixel: 0,
    };
    if unsafe { ioctl(fd, TIOCSWINSZ, &size) } < 0 {
        Err(io::Error::last_os_error().into())
    } else {
        Ok(())
    }
}
#[repr(C)]
struct Winsize {
    rows: u16,
    cols: u16,
    xpixel: u16,
    ypixel: u16,
}
#[cfg(target_os = "macos")]
const TIOCSWINSZ: u64 = 0x8008_7467;
#[cfg(target_os = "linux")]
const TIOCSWINSZ: u64 = 0x5414;
const SIGWINCH: i32 = 28;
#[cfg_attr(target_os = "linux", link(name = "util"))]
extern "C" {
    fn openpty(
        master: *mut i32,
        slave: *mut i32,
        name: *mut i8,
        termios: *const std::ffi::c_void,
        winsize: *const std::ffi::c_void,
    ) -> i32;
    fn dup(fd: i32) -> i32;
    fn close(fd: i32) -> i32;
    fn setsid() -> i32;
    fn setpgid(pid: i32, pgid: i32) -> i32;
    fn kill(pid: i32, signal: i32) -> i32;
    fn fcntl(fd: i32, cmd: i32, ...) -> i32;
    fn ioctl(fd: i32, request: u64, ...) -> i32;
}
