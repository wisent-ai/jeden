//! One shell running on a real terminal device, and the framed conversation
//! this workshop has with it.
//!
//! Split out of `runtime_ops/host/pty.rs`, which had grown past the module
//! line cap.

use super::super::super::platform::{native, ProcessSignal, PtySession};
use super::super::super::{BoundedOutput, OperationContext, OperationProgress};
use super::{
    PtyError, PtySessionMetadata, PtySessionState, DEFAULT_PTY_COLS, DEFAULT_PTY_ROWS, POLL,
};
use crate::tool_runtime::runtime_ops::host::pty::PtyResult;
use std::io;
use std::path::Path;
use std::thread;
use std::time::{Duration, Instant};

pub(super) struct PtyProcess {
    session: Box<dyn PtySession>,
    sequence: u64,
    pub(super) metadata: PtySessionMetadata,
}

impl PtyProcess {
    pub(super) fn spawn(cwd: &Path, id_sequence: u64) -> Result<Self, String> {
        let mut session = native()
            .spawn_shell(cwd, DEFAULT_PTY_COLS, DEFAULT_PTY_ROWS)
            .map_err(|error| error.to_string())?;
        let group = session.process_id();
        let (startup, ready_marker) = native().startup_handshake();
        session
            .write_all(startup)
            .map_err(|error| error.to_string())?;
        wait_for_bytes(session.as_mut(), ready_marker)?;
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

    pub(super) fn alive(&mut self) -> bool {
        self.session.alive().unwrap_or(false)
    }

    pub(super) fn execute(
        &mut self,
        context: &OperationContext<'_>,
        input: &str,
        reset: bool,
    ) -> Result<(PtyResult, bool), String> {
        self.sequence = self.sequence.wrapping_add(1);
        let frame = native().command_frame(input, self.session.process_id(), self.sequence);
        let marker = frame.marker;
        self.session
            .write_all(&frame.bytes)
            .map_err(|error| error.to_string())?;
        let mut output =
            BoundedOutput::new("pty", context.output_limits(), context.artifacts().clone());
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
                        cancelled: true,
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

    pub(super) fn resize(&mut self, cols: u16, rows: u16) -> Result<(), PtyError> {
        self.session
            .resize(cols, rows)
            .map_err(|error| PtyError::System(error.to_string()))?;
        self.metadata.cols = cols;
        self.metadata.rows = rows;
        Ok(())
    }
    pub(super) fn terminate(&mut self) {
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

/// Read until the startup marker arrives. The shell prints it once it is
/// ready; a shell that never prints it has failed, and its exit ends the
/// read below with an error rather than a guess.
fn wait_for_bytes(session: &mut dyn PtySession, marker: &[u8]) -> Result<(), String> {
    let mut pending = Vec::with_capacity(4096);
    loop {
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
