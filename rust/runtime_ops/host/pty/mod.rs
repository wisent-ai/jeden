use super::super::{OperationContext, OutputCapture};
use std::error::Error;
use std::fmt;
use std::path::Path;
// Native winsize dimensions are unsigned shorts, not an application tuning cap:
// https://man7.org/linux/man-pages/man2/TIOCSWINSZ.2const.html
pub const MIN_PTY_COLS: u16 = std::num::NonZeroU16::MIN.get();
pub const MAX_PTY_COLS: u16 = u16::MAX;
pub const MIN_PTY_ROWS: u16 = std::num::NonZeroU16::MIN.get();
pub const MAX_PTY_ROWS: u16 = u16::MAX;
mod control;
mod process;
mod registry;

use process::PtyProcess;

// VT100 standard screen geometry. Source: https://vt100.net/docs/vt100-ug/chapter1.html
pub(super) const DEFAULT_PTY_COLS: u16 = 80;
// VT100 standard screen geometry. Source: https://vt100.net/docs/vt100-ug/chapter1.html
pub(super) const DEFAULT_PTY_ROWS: u16 = 24;


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
    let slot = registry::scope(scope)?;
    let mut owned = slot.acquire(context)?;
    registry::check_current(&slot)?;
    if reset {
        if let Some(old) = owned.take() {
            registry::finish(&old.control, PtySessionState::Cancelled);
        }
    }
    if let Some(existing) = owned.as_mut() {
        if !existing.alive()? {
            let old = owned.take().expect("checked PTY process disappeared while scope was locked");
            registry::finish(&old.control, PtySessionState::Ended);
        }
    }
    if owned.is_none() {
        let mut session = PtyProcess::spawn(&canonical_cwd)?;
        registry::register(&slot, &session.control)?;
        if let Err(error) = session.initialize(Some(context)) {
            registry::finish(&session.control, if context.cancellation().is_cancelled() {
                PtySessionState::Cancelled
            } else {
                PtySessionState::Ended
            });
            return Err(error);
        }
        *owned = Some(session);
    }
    let session = owned.as_mut().expect("initialized PTY process disappeared while scope was locked");
    let result = session.execute(context, input, reset);
    match result {
        Ok((result, true)) => Ok(result),
        Ok((result, false)) => {
            let session = owned.take().expect("executed PTY process disappeared while scope was locked");
            registry::finish(&session.control, if result.cancelled {
                PtySessionState::Cancelled
            } else {
                PtySessionState::Ended
            });
            Ok(result)
        }
        Err(error) => {
            let session = owned.take().expect("failed PTY process disappeared while scope was locked");
            registry::finish(&session.control, PtySessionState::Ended);
            Err(error)
        }
    }
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
    let shared = registry::session(context, session_id)?;
    let result = {
        let mut control = control::lock(&shared).map_err(PtyError::System)?;
        if context.cancellation().is_cancelled() {
            return Err(PtyError::OperationCancelled);
        }
        control.resize(cols, rows).map(|()| control.metadata.clone())
    };
    if matches!(&result, Err(PtyError::SessionEnded { .. })) {
        registry::finish(&shared, PtySessionState::Ended);
    }
    result
}

fn validate_dimensions(cols: u16, rows: u16) -> Result<(), PtyError> {
    if !(MIN_PTY_COLS..=MAX_PTY_COLS).contains(&cols)
        || !(MIN_PTY_ROWS..=MAX_PTY_ROWS).contains(&rows)
    {
        return Err(PtyError::InvalidDimensions { cols, rows });
    }
    Ok(())
}

pub fn probe(cwd: &Path) -> Result<(), String> {
    let mut session = PtyProcess::spawn(cwd)?;
    let result = session.initialize(None);
    session.terminate();
    result
}

pub fn teardown_scope(scope: &Path) {
    registry::teardown(scope);
}
