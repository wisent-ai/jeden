//! Process groups, PTYs and desktop opening on Unix, through libc's own
//! functions and constants. Signal numbers, fcntl commands, open flags and the
//! TIOCSWINSZ request were written here by hand as numbers: they differ
//! between platforms (O_NONBLOCK is 0x4 on macOS and 0o4000 on Linux, so the
//! hand-written 4 left a Linux PTY master blocking), and libc is the
//! declaration of each for the platform being built.

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

/// The C return value `rc` as a result: libc functions answer a negative
/// value on failure and leave the cause in errno.
fn checked(rc: libc::c_int) -> io::Result<libc::c_int> {
    if rc.is_negative() {
        Err(io::Error::last_os_error())
    } else {
        Ok(rc)
    }
}

/// The process group a spawned child leads: its own pid, as `pid_t`.
fn group_of(child: &Child) -> Result<libc::pid_t, PlatformError> {
    libc::pid_t::try_from(child.id()).map_err(|error| {
        PlatformError::Process(format!(
            "process {} has no pid_t process group: {error}",
            child.id()
        ))
    })
}

/// Send `signal` to every process of `group`. A group that is already gone
/// (ESRCH) has nothing left to signal, which is what the signal was for.
fn signal_group(group: libc::pid_t, signal: ProcessSignal) -> Result<(), PlatformError> {
    let number = match signal {
        ProcessSignal::Interrupt => libc::SIGINT,
        ProcessSignal::Terminate => libc::SIGTERM,
        ProcessSignal::Kill => libc::SIGKILL,
    };
    match checked(unsafe { libc::kill(-group, number) }) {
        Ok(_signalled) => Ok(()),
        Err(error) if error.raw_os_error() == Some(libc::ESRCH) => Ok(()),
        Err(error) => Err(error.into()),
    }
}

struct UnixProcessTree {
    group: libc::pid_t,
}
impl ProcessTree for UnixProcessTree {
    fn signal(&mut self, signal: ProcessSignal) -> Result<(), PlatformError> {
        signal_group(self.group, signal)
    }
}

impl ProcessPlatform for UnixPlatform {
    fn configure_command(&self, command: &mut Command) -> Result<(), PlatformError> {
        unsafe {
            // The child leads a process group named by its own pid, so the
            // whole tree it starts can be signalled as one.
            command.pre_exec(|| {
                let own = libc::getpid();
                checked(libc::setpgid(own, own))?;
                Ok(())
            });
        }
        Ok(())
    }
    fn attach_process_tree(&self, child: &Child) -> Result<Box<dyn ProcessTree>, PlatformError> {
        Ok(Box::new(UnixProcessTree {
            group: group_of(child)?,
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
    group: libc::pid_t,
}
impl PtySession for UnixPtySession {
    fn process_id(&self) -> u32 {
        self.child.id()
    }
    fn write_all(&mut self, bytes: &[u8]) -> io::Result<()> {
        self.master
            .write_all(bytes)
            .and_then(|()| self.master.flush())
    }
    fn take_output(&mut self) -> Result<Box<dyn Read + Send>, PlatformError> {
        // The master was opened non-blocking for the old polling reads; the
        // reader thread blocks on it instead.
        let fd = duplicate(self.master_fd())?;
        if let Err(error) = set_blocking(fd, true) {
            unsafe { libc::close(fd) };
            return Err(error);
        }
        Ok(Box::new(unsafe { File::from_raw_fd(fd) }))
    }
    fn resize(&mut self, cols: u16, rows: u16) -> Result<(), PlatformError> {
        set_window_size(self.master_fd(), cols, rows)?;
        set_window_size(self.slave_fd(), cols, rows)?;
        checked(unsafe { libc::kill(-self.group, libc::SIGWINCH) })?;
        Ok(())
    }
    fn alive(&mut self) -> Result<bool, PlatformError> {
        Ok(self.child.try_wait()?.is_none())
    }
    fn exit_status(&mut self) -> Result<Option<ExitStatus>, PlatformError> {
        Ok(self.child.try_wait()?)
    }
    fn signal(&mut self, signal: ProcessSignal) -> Result<(), PlatformError> {
        signal_group(self.group, signal)
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
    /// End the shell and reap it. A drop has no caller to refuse to, so a
    /// signal or reap that fails is said on stderr instead of being dropped,
    /// and the reap says what it waits for while it waits.
    fn drop(&mut self) {
        let pid = self.child.id();
        if let Err(error) = self.signal(ProcessSignal::Kill) {
            eprintln!("jeden: the PTY shell {pid} could not be killed: {error}");
            return;
        }
        let reaped = stado_wait::blocking(
            stado_wait::Kind::Process,
            format!("the PTY shell {pid} to end"),
            "this machine",
            || self.child.wait(),
        );
        if let Err(error) = reaped {
            eprintln!("jeden: the PTY shell {pid} could not be reaped: {error}");
        }
    }
}

impl PtyPlatform for UnixPlatform {
    fn spawn_shell(
        &self,
        cwd: &Path,
        cols: u16,
        rows: u16,
    ) -> Result<Box<dyn PtySession>, PlatformError> {
        // openpty writes both descriptors before they are read.
        let (mut master, mut slave): (libc::c_int, libc::c_int) = Default::default();
        // macOS declares the termios and winsize arguments mutable and Linux
        // const; a null mutable pointer is accepted by both.
        checked(unsafe {
            libc::openpty(
                &mut master,
                &mut slave,
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                std::ptr::null_mut(),
            )
        })?;
        if let Err(error) =
            set_window_size(master, cols, rows).and_then(|()| set_window_size(slave, cols, rows))
        {
            unsafe {
                libc::close(master);
                libc::close(slave);
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
                checked(libc::setsid())?;
                Ok(())
            });
        }
        let child = command.spawn()?;
        let group = group_of(&child)?;
        let master_file = unsafe { File::from_raw_fd(master) };
        let slave_file = unsafe { File::from_raw_fd(slave) };
        set_blocking(master, false)?;
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
    Ok(checked(unsafe { libc::dup(fd) })?)
}

/// Clear (`blocking`) or set O_NONBLOCK on `fd`, keeping its other flags.
fn set_blocking(fd: RawFd, blocking: bool) -> Result<(), PlatformError> {
    let flags = checked(unsafe { libc::fcntl(fd, libc::F_GETFL) })?;
    let next = if blocking {
        flags & !libc::O_NONBLOCK
    } else {
        flags | libc::O_NONBLOCK
    };
    checked(unsafe { libc::fcntl(fd, libc::F_SETFL, next) })?;
    Ok(())
}

fn set_window_size(fd: RawFd, cols: u16, rows: u16) -> Result<(), PlatformError> {
    // The pixel size is left unknown, as a terminal that reports none does.
    let mut size: libc::winsize = unsafe { std::mem::zeroed() };
    size.ws_row = rows;
    size.ws_col = cols;
    checked(unsafe { libc::ioctl(fd, libc::TIOCSWINSZ, &size) })?;
    Ok(())
}
