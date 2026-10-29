//! One interpreter process, and the framed conversation this workshop holds
//! with it.

use super::super::platform::PipeReader;
use super::super::{BoundedOutput, OperationProgress};
use super::control::{self, SharedControl};
use super::frame::{self, Frame, Stream};
use super::output::{drain_kernel_stderr, finish_kernel, PIPE_READ_BYTES};
use super::{KernelLanguage, KernelResult};
use crate::tool_runtime::runtime_ops::context::OperationContext;
use serde_json::{json, Value};
use std::io::{self, Write};
use std::path::Path;
use std::process::ChildStdin;
use std::sync::Arc;

pub(super) struct KernelProcess {
    pub(super) control: SharedControl,
    stdin: ChildStdin,
    stdout: Box<dyn PipeReader>,
    stderr: Box<dyn PipeReader>,
    language: KernelLanguage,
    pending: Vec<u8>,
    /// The id the running evaluation was sent under, fresh for each one;
    /// frames under any other id are an earlier, interrupted evaluation's and
    /// are skipped.
    current: Option<String>,
    pub(super) registered: Option<Arc<super::KernelKey>>,
}

impl KernelProcess {
    pub(super) fn spawn(
        language: KernelLanguage,
        cwd: &Path,
        grant: &super::ExecutionGrant,
    ) -> Result<Self, String> {
        let launched = super::launch::spawn(language, cwd, grant)?;
        Ok(Self {
            control: launched.control,
            registered: None,
            stdin: launched.stdin,
            stdout: launched.stdout,
            stderr: launched.stderr,
            language,
            pending: Vec::new(),
            current: None,
        })
    }

    /// Whether the kernel still runs; its exit is kept when it has ended, and
    /// a status that cannot be read is refused with its cause.
    pub(super) fn alive(&mut self) -> Result<bool, String> {
        control::lock(&self.control)?.alive()
    }

    pub(super) fn evaluate(
        &mut self,
        context: &OperationContext<'_>,
        code: &str,
        reset: bool,
    ) -> Result<(KernelResult, bool), String> {
        control::lock(&self.control)?.require_live()?;
        if context.cancellation().is_cancelled() || control::lock(&self.control)?.cancelled() {
            return Err(format!(
                "{} kernel evaluation cancelled before code was written",
                self.language.label()
            ));
        }
        let id = uuid::Uuid::new_v4().to_string();
        let request = json!({"id": id, "code": code});
        self.current = Some(id);
        serde_json::to_writer(&mut self.stdin, &request).map_err(|error| error.to_string())?;
        self.stdin
            .write_all(b"\n")
            .and_then(|_| self.stdin.flush())
            .map_err(|error| error.to_string())?;
        let capture =
            |name| BoundedOutput::new(name, context.output_limits(), context.artifacts().clone());
        let (mut stdout, mut stderr) = (capture("kernel-stdout"), capture("kernel-stderr"));
        let mut display = capture("kernel-display");
        let mut display_mime = None;
        let mut progress_total: Option<u64> = None;
        loop {
            drain_kernel_stderr(&mut self.stderr, &mut stderr, || {
                Ok(context.cancellation().is_cancelled()
                    || control::lock(&self.control)?.cancelled())
            })?;
            if context.cancellation().is_cancelled() || control::lock(&self.control)?.cancelled() {
                let said = match self.interrupt() {
                    Ok(()) => "kernel evaluation cancelled".to_string(),
                    Err(cause) => format!("kernel evaluation cancelled; {cause}"),
                };
                return Ok((
                    finish_kernel(
                        false,
                        true,
                        reset,
                        stdout,
                        stderr,
                        display,
                        display_mime,
                        Some(said),
                    )?,
                    false,
                ));
            }
            let Some(answer) = self.next_frame()? else {
                if context.cancellation().is_cancelled()
                    || control::lock(&self.control)?.cancelled()
                {
                    continue;
                }
                if !self.alive()? {
                    drain_kernel_stderr(&mut self.stderr, &mut stderr, || {
                        Ok(context.cancellation().is_cancelled()
                            || control::lock(&self.control)?.cancelled())
                    })?;
                    let internal = stderr.finish().map_err(|e| e.to_string())?;
                    let how = control::lock(&self.control)?
                        .exit_status()
                        .ok_or_else(|| {
                            format!(
                                "{} kernel reported an exit without its observed process status",
                                self.language.label()
                            )
                        })?
                        .to_string();
                    return Err(format!(
                        "{} kernel exited before response ({how}): {}",
                        self.language.label(),
                        internal.text
                    ));
                }
                continue;
            };
            let answer_id = frame::id_of(&answer).ok_or_else(|| {
                format!("kernel protocol frame has no string evaluation id: {answer}")
            })?;
            if Some(answer_id) != self.current.as_deref() {
                continue;
            }
            match frame::read(&answer)? {
                Frame::Chunk { stream, data, mime } => {
                    let bytes = data.as_bytes();
                    match stream {
                        Stream::Stdout => stdout.write_chunk(bytes),
                        Stream::Stderr => stderr.write_chunk(bytes),
                        Stream::Display => {
                            if display_mime.is_none() {
                                display_mime = mime.map(str::to_owned);
                            }
                            display.write_chunk(bytes)
                        }
                    }
                    .map_err(|e| e.to_string())?;
                    let written = bytes.len() as u64;
                    let total = match progress_total {
                        Some(before) => before.saturating_add(written),
                        None => written,
                    };
                    progress_total = Some(total);
                    context.progress(OperationProgress {
                        stream: "kernel",
                        bytes: written,
                        total_bytes: total,
                    });
                }
                Frame::Done { ok, error } => {
                    return Ok((
                        finish_kernel(
                            ok,
                            false,
                            reset,
                            stdout,
                            stderr,
                            display,
                            display_mime,
                            error.map(str::to_owned),
                        )?,
                        true,
                    ));
                }
            }
        }
    }

    /// The next complete frame: one already buffered, or the next bytes the
    /// kernel writes. `None` when the read was woken by a cancellation, or when
    /// the kernel exited. Closing the protocol stream while still running is
    /// a protocol refusal, never a reason to wait for an unrelated process exit.
    fn next_frame(&mut self) -> Result<Option<Value>, String> {
        if let Some(frame) = self.buffered_frame()? {
            return Ok(Some(frame));
        }
        let mut chunk = [Default::default(); PIPE_READ_BYTES];
        match self.stdout.read_blocking(&mut chunk) {
            Ok(count) => {
                let read = &chunk[..count];
                if read.is_empty() {
                    if control::lock(&self.control)?.cancelled() {
                        return Ok(None);
                    }
                    if self.alive()? {
                        return Err(format!(
                            "{} kernel closed protocol stdout before completing the evaluation: process {} still runs, with {} unfinished frame bytes",
                            self.language.label(),
                            control::lock(&self.control)?.pid(),
                            self.pending.len()
                        ));
                    }
                    return Ok(None);
                }
                self.pending.extend_from_slice(read);
            }
            Err(error) if error.kind() == io::ErrorKind::Interrupted => return Ok(None),
            Err(error) => return Err(error.to_string()),
        }
        self.buffered_frame()
    }

    fn buffered_frame(&mut self) -> Result<Option<Value>, String> {
        let Some(end) = self.pending.iter().position(|byte| *byte == b'\n') else {
            return Ok(None);
        };
        let line: Vec<u8> = self.pending.drain(..=end).collect();
        serde_json::from_slice(&line)
            .map(Some)
            .map_err(|error| format!("invalid kernel protocol frame: {error}"))
    }

    /// The interrupted evaluation still answers with a `done` frame under its
    /// own id; the next evaluation skips frames that are not its own, so
    /// nothing here waits for it.
    fn interrupt(&mut self) -> Result<(), String> {
        control::lock(&self.control)?.interrupt()
    }
}

impl crate::tool_runtime::runtime_ops::session::PersistentProcess for KernelProcess {
    fn retire(&mut self, cancelled: bool) {
        if let Some(key) = &self.registered {
            super::registry::finish(key, &self.control, cancelled);
        } else {
            control::retire(&self.control, cancelled);
        }
    }
}
