//! One shell running on a real terminal device, and the framed conversation
//! this workshop has with it.
//!
//! Split out of `runtime_ops/host/pty.rs`, which had grown past the module
//! line cap.

use super::super::super::platform::{native, threaded_pipe, PipeReader, ProcessSignal};
use super::super::super::{BoundedOutput, OperationContext, OperationProgress};
use super::{PtySessionMetadata, PtySessionState, DEFAULT_PTY_COLS, DEFAULT_PTY_ROWS};
use crate::tool_runtime::runtime_ops::host::pty::PtyResult;
use std::io;
use std::path::Path;
use std::sync::{Arc, Mutex};
use super::control::{self, PtyControl, SharedControl};

/// How many bytes one read of the terminal takes: the size POSIX guarantees
/// a pipe writes whole. <https://pubs.opengroup.org/onlinepubs/9799919799/basedefs/limits.h.html>
const READ_BYTES: usize = libc::PIPE_BUF;

pub(super) struct PtyProcess {
    // The terminal's output, read on its own thread: reads block until bytes
    // arrive, the shell goes away, or an announced cancellation wakes them.
    output: Box<dyn PipeReader>,
    pub(super) control: SharedControl,
}

impl PtyProcess {
    pub(super) fn spawn(cwd: &Path) -> Result<Self, String> {
        let mut session = native()
            .spawn_shell(cwd, DEFAULT_PTY_COLS, DEFAULT_PTY_ROWS)
            .map_err(|error| error.to_string())?;
        let output = threaded_pipe(session.take_output().map_err(|error| error.to_string())?);
        crate::tool_runtime::runtime_ops::wake_on_cancellation(output.waker());
        let group = session.process_id();
        let control = Arc::new(Mutex::new(PtyControl {
            session,
            wake: output.waker(),
            kill_complete: false,
            metadata: PtySessionMetadata {
                session_id: format!("pty-{group}-{}", uuid::Uuid::new_v4()),
                cols: DEFAULT_PTY_COLS,
                rows: DEFAULT_PTY_ROWS,
                state: PtySessionState::Live,
            },
        }));
        Ok(Self {
            control,
            output,
        })
    }

    pub(super) fn initialize(&mut self, context: Option<&OperationContext<'_>>) -> Result<(), String> {
        let (startup, ready_marker) = native().startup_handshake();
        {
            let mut control = control::lock(&self.control)?;
            control.require_live().map_err(|error| error.to_string())?;
            if context.is_some_and(|context| context.cancellation().is_cancelled()) {
                return Err("PTY startup cancelled before shell readiness".into());
            }
            control.session.write_all(startup).map_err(|error| error.to_string())?;
        }
        wait_for_bytes(self.output.as_mut(), ready_marker, &self.control, context)?;
        drain(self.output.as_mut())
    }

    /// A status-read failure is not evidence that the persistent shell ended.
    pub(super) fn alive(&mut self) -> Result<bool, String> {
        control::lock(&self.control)?.alive()
    }

    pub(super) fn execute(
        &mut self,
        context: &OperationContext<'_>,
        input: &str,
        reset: bool,
    ) -> Result<(PtyResult, bool), String> {
        // A marker no earlier command used, so an interrupted command's late
        // marker is never taken for this one's.
        let sequence = uuid::Uuid::new_v4().as_u64_pair().0;
        let frame = {
            let mut control = control::lock(&self.control)?;
            control.require_live().map_err(|error| error.to_string())?;
            if context.cancellation().is_cancelled() {
                return Err(format!("PTY session '{}' execution cancelled before command was written", control.metadata.session_id));
            }
            let frame = native().command_frame(input, control.session.process_id(), sequence);
            control.session.write_all(&frame.bytes).map_err(|error| error.to_string())?;
            frame
        };
        let marker = frame.marker;
        let mut output =
            BoundedOutput::new("pty", context.output_limits(), context.artifacts().clone());
        let mut pending = Vec::new();
        let mut progress_total: Option<u64> = None;
        loop {
            let state = control::lock(&self.control)?.metadata.state;
            if state == PtySessionState::Ended {
                return Err("PTY shell ended before command completed".into());
            }
            if context.cancellation().is_cancelled() || state == PtySessionState::Cancelled {
                let mut session = self.metadata()?;
                session.state = PtySessionState::Cancelled;
                // These bytes were already read but held back in case they
                // completed the frame marker. Cancellation must not lose them.
                output.write_chunk(&pending).map_err(|error| error.to_string())?;
                let mut capture = output.finish().map_err(|e| e.to_string())?;
                let interrupt = {
                    let mut control = control::lock(&self.control)?;
                    if control.metadata.state == PtySessionState::Live {
                        control.session.signal(ProcessSignal::Interrupt)
                    } else {
                        Ok(())
                    }
                };
                if let Err(error) = interrupt {
                    capture.text.push_str(&format!("\njeden: the shell could not be interrupted: {error}"));
                }
                return Ok((
                    PtyResult {
                        ok: false,
                        cancelled: true,
                        reset,
                        code: None,
                        output: capture,
                        session,
                    },
                    false,
                ));
            }
            let mut chunk = [Default::default(); READ_BYTES];
            match self.output.read_blocking(&mut chunk) {
                Ok(count) => {
                    let read = &chunk[..count];
                    if read.is_empty() {
                        if control::lock(&self.control)?.metadata.state == PtySessionState::Cancelled {
                            continue;
                        }
                        return Err("PTY shell exited before command completed".into());
                    }
                    pending.extend_from_slice(read);
                    if let Some(position) = find_bytes(&pending, marker.as_bytes()) {
                        let suffix = &pending[position + marker.len()..];
                        // The marker line ends at its newline; until it has
                        // arrived the exit code may be cut short.
                        if let Some(line_end) = suffix.iter().position(|byte| *byte == b'\n') {
                            let code = parse_marker_code(&suffix[..line_end])?;
                            output
                                .write_chunk(&pending[..position])
                                .map_err(|e| e.to_string())?;
                            let capture = output.finish().map_err(|e| e.to_string())?;
                            return Ok((
                                PtyResult {
                                    ok: code == libc::EXIT_SUCCESS,
                                    cancelled: false,
                                    reset,
                                    code: Some(code),
                                    output: capture,
                                    session: self.metadata()?,
                                },
                                true,
                            ));
                        }
                    } else if let Some(emit) = pending.len().checked_sub(marker.len()) {
                        // Everything but the last marker's length is output:
                        // the marker can only begin in what is kept.
                        output
                            .write_chunk(&pending[..emit])
                            .map_err(|e| e.to_string())?;
                        pending.drain(..emit);
                    }
                    let written = count as u64;
                    let total = match progress_total {
                        Some(before) => before.saturating_add(written),
                        None => written,
                    };
                    progress_total = Some(total);
                    context.progress(OperationProgress {
                        stream: "pty",
                        bytes: written,
                        total_bytes: total,
                    });
                }
                // A cancellation (this turn's or another's) woke the read; the
                // loop's first check decides which.
                Err(error) if error.kind() == io::ErrorKind::Interrupted => {}
                Err(error) => return Err(error.to_string()),
            }
        }
    }

    pub(super) fn metadata(&self) -> Result<PtySessionMetadata, String> {
        Ok(control::lock(&self.control)?.metadata.clone())
    }

    pub(super) fn terminate(&mut self) {
        match self.control.lock() {
            Ok(mut control) => { control.terminate(PtySessionState::Ended); }
            Err(poisoned) => {
                eprintln!("jeden: PTY control lock poisoned during probe cleanup: {poisoned}; recovering its owned process to terminate it");
                poisoned.into_inner().terminate(PtySessionState::Ended);
            }
        }
    }
}

impl crate::tool_runtime::runtime_ops::session::PersistentProcess for PtyProcess {
    fn retire(&mut self, cancelled: bool) {
        super::registry::finish(&self.control, if cancelled {
            PtySessionState::Cancelled
        } else {
            PtySessionState::Ended
        });
    }
}

/// Read until the startup marker arrives. The shell prints it once it is
/// ready; a shell that never prints it has failed, and its exit ends the
/// read below with an error rather than a guess.
fn wait_for_bytes(
    output: &mut dyn PipeReader,
    marker: &[u8],
    shared: &SharedControl,
    context: Option<&OperationContext<'_>>,
) -> Result<(), String> {
    let mut pending = Vec::new();
    loop {
        control::lock(shared)?.require_live().map_err(|error| error.to_string())?;
        if context.is_some_and(|context| context.cancellation().is_cancelled()) {
            return Err("PTY startup cancelled before shell readiness".into());
        }
        let mut buffer = [Default::default(); READ_BYTES];
        match output.read_blocking(&mut buffer) {
            Ok(count) => {
                let read = &buffer[..count];
                if read.is_empty() {
                    control::lock(shared)?.require_live().map_err(|error| error.to_string())?;
                    return Err("PTY shell exited before it was ready".into());
                }
                pending.extend_from_slice(read);
                if find_bytes(&pending, marker).is_some() {
                    return Ok(());
                }
                // Only a marker that began in the last marker's length of
                // what was read can still complete.
                if let Some(seen) = pending.len().checked_sub(marker.len()) {
                    pending.drain(..seen);
                }
            }
            // All terminal readers are woken on cancellation. Only this
            // session's lifecycle and this operation decide whether to stop.
            Err(error) if error.kind() == io::ErrorKind::Interrupted => {}
            Err(error) => return Err(error.to_string()),
        }
    }
}

fn find_bytes(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    haystack
        .windows(needle.len())
        .position(|window| window == needle)
}

/// The exit code the marker line states after its `:`, or why it states none.
fn parse_marker_code(line: &[u8]) -> Result<i32, String> {
    let text = String::from_utf8_lossy(line);
    let code = text.trim_start_matches(':').trim();
    code.parse()
        .map_err(|error| format!("the PTY shell's completion marker states {code:?}, not an exit code: {error}"))
}

/// Read away what the shell printed while starting, up to what has arrived;
/// a read that fails is refused with its cause.
fn drain(output: &mut dyn PipeReader) -> Result<(), String> {
    let mut buffer = [Default::default(); READ_BYTES];
    loop {
        match output.read_available(&mut buffer) {
            Ok(count) if buffer[..count].is_empty() => return Ok(()),
            Ok(_) => {}
            Err(error) if error.kind() == io::ErrorKind::WouldBlock => return Ok(()),
            Err(error) if error.kind() == io::ErrorKind::Interrupted => {}
            Err(error) => return Err(format!("the PTY shell's startup output could not be read: {error}")),
        }
    }
}
