use std::ffi::OsStr;
use std::fmt;
use std::fs::File;
use std::io;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, ExitStatus};

#[cfg(target_os = "linux")]
mod linux;
#[cfg(target_os = "macos")]
mod macos;
#[cfg(any(target_os = "macos", target_os = "linux"))]
mod unix;
pub mod update;
#[cfg(windows)]
mod windows;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum UnsupportedReason {
    OperatingSystem,
    KernelFacility,
    RuntimeApiUnavailable,
    FilesystemCapability,
}

struct ThreadPipeReader {
    receiver: std::sync::mpsc::Receiver<io::Result<Vec<u8>>>,
    pending: Vec<u8>,
}

impl PipeReader for ThreadPipeReader {
    fn read_available(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
        if self.pending.is_empty() {
            match self.receiver.try_recv() {
                Ok(Ok(bytes)) => self.pending = bytes,
                Ok(Err(error)) => return Err(error),
                Err(std::sync::mpsc::TryRecvError::Empty) => {
                    return Err(io::ErrorKind::WouldBlock.into())
                }
                Err(std::sync::mpsc::TryRecvError::Disconnected) => return Ok(0),
            }
        }
        let count = buffer.len().min(self.pending.len());
        buffer[..count].copy_from_slice(&self.pending[..count]);
        self.pending.drain(..count);
        Ok(count)
    }
}

pub(super) fn threaded_pipe(mut pipe: Box<dyn std::io::Read + Send>) -> Box<dyn PipeReader> {
    use std::io::Read;
    let (sender, receiver) = std::sync::mpsc::channel();
    std::thread::spawn(move || loop {
        let mut bytes = vec![0; 8192];
        match pipe.read(&mut bytes) {
            Ok(0) => break,
            Ok(count) => {
                bytes.truncate(count);
                if sender.send(Ok(bytes)).is_err() {
                    break;
                }
            }
            Err(error) => {
                let _ = sender.send(Err(error));
                break;
            }
        }
    });
    Box::new(ThreadPipeReader {
        receiver,
        pending: Vec::new(),
    })
}
#[derive(Debug)]
pub enum PlatformError {
    Unsupported {
        feature: &'static str,
        target: &'static str,
        reason: UnsupportedReason,
    },
    Io(io::Error),
    Process(String),
}

impl PlatformError {
    pub fn unsupported(feature: &'static str, reason: UnsupportedReason) -> Self {
        Self::Unsupported {
            feature,
            target: std::env::consts::OS,
            reason,
        }
    }
}

impl fmt::Display for PlatformError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Unsupported {
                feature,
                target,
                reason,
            } => write!(f, "{feature} is unsupported on {target}: {reason:?}"),
            Self::Io(error) => error.fmt(f),
            Self::Process(message) => f.write_str(message),
        }
    }
}
impl std::error::Error for PlatformError {}
impl From<io::Error> for PlatformError {
    fn from(value: io::Error) -> Self {
        Self::Io(value)
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ProcessSignal {
    Interrupt,
    Terminate,
    Kill,
}

pub trait ProcessTree: Send {
    fn signal(&mut self, signal: ProcessSignal) -> Result<(), PlatformError>;
}

pub trait PipeReader: Send {
    fn read_available(&mut self, buffer: &mut [u8]) -> io::Result<usize>;
}

pub trait ProcessPlatform: Sync {
    fn configure_command(&self, command: &mut Command) -> Result<(), PlatformError>;
    fn attach_process_tree(&self, child: &Child) -> Result<Box<dyn ProcessTree>, PlatformError>;
    fn pipe_reader(
        &self,
        pipe: Box<dyn std::io::Read + Send>,
    ) -> Result<Box<dyn PipeReader>, PlatformError>;
}

pub trait PtySession: Send {
    fn process_id(&self) -> u32;
    fn write_all(&mut self, bytes: &[u8]) -> io::Result<()>;
    fn read_available(&mut self, buffer: &mut [u8]) -> io::Result<usize>;
    fn resize(&mut self, cols: u16, rows: u16) -> Result<(), PlatformError>;
    fn alive(&mut self) -> Result<bool, PlatformError>;
    fn exit_status(&mut self) -> Result<Option<ExitStatus>, PlatformError>;
    fn signal(&mut self, signal: ProcessSignal) -> Result<(), PlatformError>;
}

pub struct PtyCommandFrame {
    pub marker: String,
    pub bytes: Vec<u8>,
}

pub trait PtyPlatform: Sync {
    fn spawn_shell(
        &self,
        cwd: &Path,
        cols: u16,
        rows: u16,
    ) -> Result<Box<dyn PtySession>, PlatformError>;
    fn startup_handshake(&self) -> (&'static [u8], &'static [u8]);
    fn command_frame(&self, input: &str, process_id: u32, sequence: u64) -> PtyCommandFrame;
}

pub trait WorkspacePlatform: Sync {
    fn isolate(&self, parent: &Path, target: &Path) -> Result<&'static str, PlatformError>;
    fn snapshot(
        &self,
        parent: &Path,
        workspace: &Path,
        max_bytes: u64,
    ) -> Result<Vec<u8>, PlatformError>;
    fn apply_snapshot(
        &self,
        parent: &Path,
        snapshot: &[u8],
        max_diagnostics: u64,
    ) -> Result<(), PlatformError>;
}

pub struct SecureTemp {
    pub path: PathBuf,
    pub file: File,
}

pub trait AtomicFsPlatform: Sync {
    fn create_secure_temp(
        &self,
        directory: &Path,
        prefix: &OsStr,
    ) -> Result<SecureTemp, PlatformError>;
    fn atomic_replace(
        &self,
        staged: &Path,
        destination: &Path,
        backup: Option<&Path>,
    ) -> Result<(), PlatformError>;
}

pub trait DesktopPlatform: Sync {
    fn open_path(&self, path: &Path) -> Result<(), PlatformError>;
}

pub trait Platform:
    ProcessPlatform + PtyPlatform + WorkspacePlatform + AtomicFsPlatform + DesktopPlatform
{
}
impl<T: ProcessPlatform + PtyPlatform + WorkspacePlatform + AtomicFsPlatform + DesktopPlatform>
    Platform for T
{
}

#[cfg(target_os = "linux")]
pub(crate) use linux::NativePlatform;
#[cfg(target_os = "macos")]
pub(crate) use macos::NativePlatform;
#[cfg(windows)]
pub(crate) use windows::NativePlatform;

#[cfg(not(any(target_os = "macos", target_os = "linux", windows)))]
compile_error!("Jeden platform adapters support only macOS, Linux, and Windows");

static NATIVE: NativePlatform = NativePlatform::new();
pub fn native() -> &'static dyn Platform {
    &NATIVE
}
