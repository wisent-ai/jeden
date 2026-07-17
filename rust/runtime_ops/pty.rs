use super::{
    platform::{native, ProcessSignal, PtySession},
    BoundedOutput, OperationContext, OperationProgress, OutputCapture,
};
use std::collections::HashMap;
use std::error::Error;
use std::fmt;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::{LazyLock, Mutex, MutexGuard, TryLockError};
use std::thread;
use std::time::{Duration, Instant};

const POLL: Duration = Duration::from_millis(10);
pub const MIN_PTY_COLS: u16 = 1;
pub const MAX_PTY_COLS: u16 = 1_000;
pub const MIN_PTY_ROWS: u16 = 1;
pub const MAX_PTY_ROWS: u16 = 1_000;
const DEFAULT_PTY_COLS: u16 = 80;
const DEFAULT_PTY_ROWS: u16 = 24;

#[derive(Default)]
struct PtyRegistry {
    sessions: HashMap<PathBuf, PtyProcess>,
    terminal: HashMap<String, PtySessionState>,
    next_id: u64,
}

static SESSIONS: LazyLock<Mutex<PtyRegistry>> =
    LazyLock::new(|| Mutex::new(PtyRegistry::default()));

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PtySessionState {
    Live,
    Ended,
    Cancelled,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PtySessionMetadata {
    pub session_id: String,
    pub cols: u16,
    pub rows: u16,
    pub state: PtySessionState,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum PtyError {
    InvalidDimensions { cols: u16, rows: u16 },
    SessionNotFound { session_id: String },
    SessionEnded { session_id: String },
    SessionCancelled { session_id: String },
    OperationCancelled,
    System(String),
}

impl fmt::Display for PtyError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidDimensions { cols, rows } => write!(formatter, "invalid PTY dimensions {cols}x{rows}; cols and rows must be within {MIN_PTY_COLS}..={MAX_PTY_COLS} and {MIN_PTY_ROWS}..={MAX_PTY_ROWS}"),
            Self::SessionNotFound { session_id } => write!(formatter, "PTY session '{session_id}' does not exist"),
            Self::SessionEnded { session_id } => write!(formatter, "PTY session '{session_id}' has ended"),
            Self::SessionCancelled { session_id } => write!(formatter, "PTY session '{session_id}' was cancelled"),
            Self::OperationCancelled => formatter.write_str("PTY resize cancelled"),
            Self::System(message) => formatter.write_str(message),
        }
    }
}

impl Error for PtyError {}

pub struct PtyResult {
    pub ok: bool,
    pub timed_out: bool,
    pub cancelled: bool,
    pub reset: bool,
    pub code: Option<i32>,
    pub output: OutputCapture,
    pub session: PtySessionMetadata,
}

pub fn execute(
    context: &OperationContext<'_>,
    scope: &Path,
    cwd: &Path,
    input: &str,
    reset: bool,
    timeout: Duration,
) -> Result<PtyResult, String> {
    let child = super::untrusted_child(context, format!("{}:pty", context.operation_id()))
        .map_err(|error| error.to_string())?;
    let grant = child.execution_grant();
    let canonical_cwd = cwd
        .canonicalize()
        .map_err(|error| format!("PTY cwd unavailable: {error}"))?;
    if !grant
        .filesystem
        .read_roots
        .iter()
        .any(|root| canonical_cwd.starts_with(root))
    {
        return Err(super::GrantError::FilesystemDenied(format!(
            "PTY cwd {} is outside grant",
            canonical_cwd.display()
        ))
        .to_string());
    }
    let key = scope.to_path_buf();
    let mut registry = SESSIONS.lock().map_err(|_| "PTY registry lock poisoned")?;
    if reset {
        if let Some(mut old) = registry.sessions.remove(&key) {
            let id = old.metadata.session_id.clone();
            old.terminate();
            registry.terminal.insert(id, PtySessionState::Cancelled);
        }
    }
    let mut session = if let Some(mut existing) = registry.sessions.remove(&key) {
        if existing.alive() {
            existing
        } else {
            let id = existing.metadata.session_id.clone();
            existing.terminate();
            registry.terminal.insert(id, PtySessionState::Ended);
            spawn_registered(&mut registry, &canonical_cwd)?
        }
    } else {
        spawn_registered(&mut registry, &canonical_cwd)?
    };
    let result = session.execute(context, input, reset, timeout);
    match result {
        Ok((result, healthy)) => {
            if healthy {
                registry.sessions.insert(key, session);
            } else {
                let id = session.metadata.session_id.clone();
                session.terminate();
                registry.terminal.insert(
                    id,
                    if result.cancelled {
                        PtySessionState::Cancelled
                    } else {
                        PtySessionState::Ended
                    },
                );
            }
            Ok(result)
        }
        Err(error) => {
            let id = session.metadata.session_id.clone();
            session.terminate();
            registry.terminal.insert(id, PtySessionState::Ended);
            Err(error)
        }
    }
}

fn spawn_registered(registry: &mut PtyRegistry, cwd: &Path) -> Result<PtyProcess, String> {
    registry.next_id = registry.next_id.wrapping_add(1);
    PtyProcess::spawn(cwd, registry.next_id)
}
pub fn resize(
    context: &OperationContext<'_>,
    session_id: &str,
    cols: u16,
    rows: u16,
) -> Result<PtySessionMetadata, PtyError> {
    super::untrusted_child(context, format!("{}:pty-resize", context.operation_id()))
        .map_err(|error| PtyError::System(error.to_string()))?;
    validate_dimensions(cols, rows)?;
    let mut registry = lock_registry_cancellable(context)?;
    let scope = registry.sessions.iter().find_map(|(scope, session)| {
        (session.metadata.session_id == session_id).then(|| scope.clone())
    });
    let Some(scope) = scope else {
        return Err(match registry.terminal.get(session_id) {
            Some(PtySessionState::Ended) => PtyError::SessionEnded {
                session_id: session_id.into(),
            },
            Some(PtySessionState::Cancelled) => PtyError::SessionCancelled {
                session_id: session_id.into(),
            },
            _ => PtyError::SessionNotFound {
                session_id: session_id.into(),
            },
        });
    };
    let alive = registry
        .sessions
        .get_mut(&scope)
        .is_some_and(PtyProcess::alive);
    if !alive {
        if let Some(mut session) = registry.sessions.remove(&scope) {
            session.terminate();
        }
        registry
            .terminal
            .insert(session_id.into(), PtySessionState::Ended);
        return Err(PtyError::SessionEnded {
            session_id: session_id.into(),
        });
    }
    let session = registry
        .sessions
        .get_mut(&scope)
        .expect("live PTY scope disappeared while registry was locked");
    session.resize(cols, rows)?;
    Ok(session.metadata.clone())
}

fn validate_dimensions(cols: u16, rows: u16) -> Result<(), PtyError> {
    if !(MIN_PTY_COLS..=MAX_PTY_COLS).contains(&cols)
        || !(MIN_PTY_ROWS..=MAX_PTY_ROWS).contains(&rows)
    {
        return Err(PtyError::InvalidDimensions { cols, rows });
    }
    Ok(())
}

fn lock_registry_cancellable(
    context: &OperationContext<'_>,
) -> Result<MutexGuard<'static, PtyRegistry>, PtyError> {
    loop {
        if context.cancellation().is_cancelled() {
            return Err(PtyError::OperationCancelled);
        }
        match SESSIONS.try_lock() {
            Ok(registry) => return Ok(registry),
            Err(TryLockError::WouldBlock) => thread::sleep(POLL),
            Err(TryLockError::Poisoned(_)) => {
                return Err(PtyError::System("PTY registry lock poisoned".into()))
            }
        }
    }
}
pub fn probe(cwd: &Path) -> Result<(), String> {
    let mut session = PtyProcess::spawn(cwd, 0)?;
    session.terminate();
    Ok(())
}

pub fn teardown_scope(scope: &Path) {
    if let Ok(mut registry) = SESSIONS.lock() {
        if let Some(mut session) = registry.sessions.remove(scope) {
            let id = session.metadata.session_id.clone();
            session.terminate();
            registry.terminal.insert(id, PtySessionState::Cancelled);
        }
    }
}

struct PtyProcess {
    session: Box<dyn PtySession>,
    sequence: u64,
    metadata: PtySessionMetadata,
}

impl PtyProcess {
    fn spawn(cwd: &Path, id_sequence: u64) -> Result<Self, String> {
        let mut session = native()
            .spawn_shell(cwd, DEFAULT_PTY_COLS, DEFAULT_PTY_ROWS)
            .map_err(|error| error.to_string())?;
        let group = session.process_id();
        let (startup, ready_marker) = native().startup_handshake();
        session
            .write_all(startup)
            .map_err(|error| error.to_string())?;
        wait_for_bytes(session.as_mut(), ready_marker, Duration::from_secs(2))?;
        drain(session.as_mut());
        Ok(Self {
            session,
            sequence: 0,
            metadata: PtySessionMetadata {
                session_id: format!("pty-{group}-{id_sequence}"),
                cols: DEFAULT_PTY_COLS,
                rows: DEFAULT_PTY_ROWS,
                state: PtySessionState::Live,
            },
        })
    }

    fn alive(&mut self) -> bool {
        self.session.alive().unwrap_or(false)
    }

    fn execute(
        &mut self,
        context: &OperationContext<'_>,
        input: &str,
        reset: bool,
        timeout: Duration,
    ) -> Result<(PtyResult, bool), String> {
        self.sequence = self.sequence.wrapping_add(1);
        let frame = native().command_frame(input, self.session.process_id(), self.sequence);
        let marker = frame.marker;
        self.session
            .write_all(&frame.bytes)
            .map_err(|error| error.to_string())?;
        let mut output =
            BoundedOutput::new("pty", context.output_limits(), context.artifacts().clone());
        let deadline = context.effective_deadline(timeout);
        let mut pending = Vec::with_capacity(marker.len() + 8192);
        let mut progress_total = 0u64;
        loop {
            if context.cancellation().is_cancelled() {
                let _ = self.session.signal(ProcessSignal::Interrupt);
                let mut session = self.metadata.clone();
                session.state = PtySessionState::Cancelled;
                return Ok((
                    PtyResult {
                        ok: false,
                        timed_out: false,
                        cancelled: true,
                        reset,
                        code: None,
                        output: output.finish().map_err(|e| e.to_string())?,
                        session,
                    },
                    false,
                ));
            }
            if Instant::now() >= deadline {
                let _ = self.session.signal(ProcessSignal::Interrupt);
                let mut session = self.metadata.clone();
                session.state = PtySessionState::Ended;
                return Ok((
                    PtyResult {
                        ok: false,
                        timed_out: true,
                        cancelled: false,
                        reset,
                        code: None,
                        output: output.finish().map_err(|e| e.to_string())?,
                        session,
                    },
                    false,
                ));
            }
            let mut chunk = [0u8; 8192];
            match self.session.read_available(&mut chunk) {
                Ok(0) => {
                    if !self.alive() {
                        return Err("PTY shell exited before command completed".into());
                    }
                }
                Ok(count) => {
                    pending.extend_from_slice(&chunk[..count]);
                    if let Some(position) = find_bytes(&pending, marker.as_bytes()) {
                        output
                            .write_chunk(&pending[..position])
                            .map_err(|e| e.to_string())?;
                        let suffix = &pending[position + marker.len()..];
                        let code = parse_marker_code(suffix);
                        let capture = output.finish().map_err(|e| e.to_string())?;
                        return Ok((
                            PtyResult {
                                ok: code == Some(0),
                                timed_out: false,
                                cancelled: false,
                                reset,
                                code,
                                output: capture,
                                session: self.metadata.clone(),
                            },
                            true,
                        ));
                    }
                    let retain = marker.len().saturating_add(16);
                    if pending.len() > retain {
                        let emit = pending.len() - retain;
                        output
                            .write_chunk(&pending[..emit])
                            .map_err(|e| e.to_string())?;
                        pending.drain(..emit);
                    }
                    progress_total = progress_total.saturating_add(count as u64);
                    context.progress(OperationProgress {
                        stream: "pty",
                        bytes: count as u64,
                        total_bytes: progress_total,
                    });
                }
                Err(error) if error.kind() == io::ErrorKind::WouldBlock => thread::sleep(POLL),
                Err(error) => return Err(error.to_string()),
            }
        }
    }

    fn resize(&mut self, cols: u16, rows: u16) -> Result<(), PtyError> {
        self.session
            .resize(cols, rows)
            .map_err(|error| PtyError::System(error.to_string()))?;
        self.metadata.cols = cols;
        self.metadata.rows = rows;
        Ok(())
    }
    fn terminate(&mut self) {
        let _ = self.session.signal(ProcessSignal::Terminate);
        let until = Instant::now() + Duration::from_millis(300);
        while Instant::now() < until {
            if !self.alive() {
                return;
            }
            thread::sleep(POLL);
        }
        let _ = self.session.signal(ProcessSignal::Kill);
    }
}

fn wait_for_bytes(
    session: &mut dyn PtySession,
    marker: &[u8],
    timeout: Duration,
) -> Result<(), String> {
    let deadline = Instant::now() + timeout;
    let mut pending = Vec::with_capacity(4096);
    while Instant::now() < deadline {
        let mut buffer = [0u8; 4096];
        match session.read_available(&mut buffer) {
            Ok(count) if count > 0 => {
                pending.extend_from_slice(&buffer[..count]);
                if find_bytes(&pending, marker).is_some() {
                    return Ok(());
                }
                if pending.len() > 8192 {
                    pending.drain(..4096);
                }
            }
            Ok(_) => thread::sleep(POLL),
            Err(error) if error.kind() == io::ErrorKind::WouldBlock => thread::sleep(POLL),
            Err(error) => return Err(error.to_string()),
        }
    }
    Err("PTY shell did not complete startup handshake".into())
}
fn find_bytes(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    haystack
        .windows(needle.len())
        .position(|window| window == needle)
}
fn parse_marker_code(suffix: &[u8]) -> Option<i32> {
    let text = String::from_utf8_lossy(suffix);
    text.trim_start_matches(':')
        .lines()
        .next()?
        .trim()
        .parse()
        .ok()
}
fn drain(session: &mut dyn PtySession) {
    let mut buffer = [0u8; 4096];
    loop {
        match session.read_available(&mut buffer) {
            Ok(0) | Err(_) => break,
            Ok(_) => {}
        }
    }
}
