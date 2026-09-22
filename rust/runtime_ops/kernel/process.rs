//! One interpreter process, and the framed conversation this workshop holds
//! with it.
//!
//! Split out of `runtime_ops/kernel.rs`, which had grown past the module line
//! cap.

use super::super::platform::{native, PipeReader, ProcessSignal, ProcessTree};
use super::super::{BoundedOutput, OperationProgress};
use super::bootstrap::{JAVASCRIPT_BOOTSTRAP, PYTHON_BOOTSTRAP};
use super::{KernelLanguage, KernelResult, FRAME_LIMIT, POLL};
use crate::tool_runtime::runtime_ops::context::OperationContext;
use serde_json::{json, Value};
use std::ffi::OsStr;
use std::io::{self, Write};
use std::path::Path;
use std::process::{Child, ChildStdin, Command, Stdio};
use std::thread;
use std::time::{Duration, Instant};

pub(super) struct KernelProcess {
    child: Child,
    stdin: ChildStdin,
    stdout: Box<dyn PipeReader>,
    stderr: Box<dyn PipeReader>,
    process_tree: Box<dyn ProcessTree>,
    language: KernelLanguage,
    pending: Vec<u8>,
    sequence: u64,
}

impl KernelProcess {
    pub(super) fn spawn(
        language: KernelLanguage,
        cwd: &Path,
        grant: &super::ExecutionGrant,
    ) -> Result<Self, String> {
        let (program, args): (&OsStr, Vec<&OsStr>) = match language {
            KernelLanguage::Python => (
                OsStr::new("python3"),
                vec![
                    OsStr::new("-u"),
                    OsStr::new("-c"),
                    OsStr::new(PYTHON_BOOTSTRAP),
                ],
            ),
            KernelLanguage::JavaScript => (
                OsStr::new("node"),
                vec![OsStr::new("-e"), OsStr::new(JAVASCRIPT_BOOTSTRAP)],
            ),
        };
        let mut command = Command::new(program);
        command
            .args(args)
            .current_dir(cwd)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        command.env_clear();
        for key in &grant.process.environment {
            if let Some(value) = std::env::var_os(key) {
                command.env(key, value);
            }
        }
        native()
            .configure_command(&mut command)
            .map_err(|error| error.to_string())?;
        let mut child = command
            .spawn()
            .map_err(|error| format!("failed launching {} kernel: {error}", language.label()))?;
        let process_tree = native()
            .attach_process_tree(&child)
            .map_err(|error| error.to_string())?;
        let stdin = child.stdin.take().ok_or("kernel stdin unavailable")?;
        let stdout = native()
            .pipe_reader(Box::new(
                child.stdout.take().ok_or("kernel stdout unavailable")?,
            ))
            .map_err(|error| error.to_string())?;
        let stderr = native()
            .pipe_reader(Box::new(
                child.stderr.take().ok_or("kernel stderr unavailable")?,
            ))
            .map_err(|error| error.to_string())?;
        Ok(Self {
            child,
            stdin,
            stdout,
            stderr,
            process_tree,
            language,
            pending: Vec::with_capacity(8192),
            sequence: 0,
        })
    }

    pub(super) fn alive(&mut self) -> bool {
        self.child.try_wait().ok().flatten().is_none()
    }

    pub(super) fn evaluate(
        &mut self,
        context: &OperationContext<'_>,
        code: &str,
        reset: bool,
    ) -> Result<(KernelResult, bool), String> {
        self.sequence = self.sequence.wrapping_add(1);
        let request = json!({"id": self.sequence, "code": code});
        serde_json::to_writer(&mut self.stdin, &request).map_err(|error| error.to_string())?;
        self.stdin
            .write_all(b"\n")
            .and_then(|_| self.stdin.flush())
            .map_err(|error| error.to_string())?;
        let mut stdout = BoundedOutput::new(
            "kernel-stdout",
            context.output_limits(),
            context.artifacts().clone(),
        );
        let mut stderr = BoundedOutput::new(
            "kernel-stderr",
            context.output_limits(),
            context.artifacts().clone(),
        );
        let mut display = BoundedOutput::new(
            "kernel-display",
            context.output_limits(),
            context.artifacts().clone(),
        );
        let mut display_mime = None;
        let mut progress_total = 0u64;
        loop {
            drain_kernel_stderr(&mut self.stderr, &mut stderr)?;
            if context.cancellation().is_cancelled() {
                self.interrupt();
                return Ok((
                    finish_kernel(
                        false,
                        true,
                        reset,
                        stdout,
                        stderr,
                        display,
                        display_mime,
                        Some("kernel evaluation cancelled".into()),
                    )?,
                    false,
                ));
            }
            if let Some(frame) = self.next_frame()? {
                if frame.get("id").and_then(Value::as_u64) != Some(self.sequence) {
                    continue;
                }
                match frame.get("type").and_then(Value::as_str).unwrap_or("") {
                    "chunk" => {
                        let bytes = frame
                            .get("data")
                            .and_then(Value::as_str)
                            .unwrap_or("")
                            .as_bytes();
                        match frame.get("stream").and_then(Value::as_str).unwrap_or("") {
                            "stdout" => stdout.write_chunk(bytes),
                            "stderr" => stderr.write_chunk(bytes),
                            "display" => {
                                if display_mime.is_none() {
                                    display_mime = frame
                                        .get("mime")
                                        .and_then(Value::as_str)
                                        .map(ToString::to_string);
                                }
                                display.write_chunk(bytes)
                            }
                            _ => continue,
                        }
                        .map_err(|e| e.to_string())?;
                        progress_total = progress_total.saturating_add(bytes.len() as u64);
                        context.progress(OperationProgress {
                            stream: "kernel",
                            bytes: bytes.len() as u64,
                            total_bytes: progress_total,
                        });
                    }
                    "done" => {
                        let ok = frame.get("ok").and_then(Value::as_bool).unwrap_or(false);
                        let error = frame
                            .get("error")
                            .and_then(Value::as_str)
                            .map(ToString::to_string);
                        return Ok((
                            finish_kernel(
                                ok,
                                false,
                                reset,
                                stdout,
                                stderr,
                                display,
                                display_mime,
                                error,
                            )?,
                            true,
                        ));
                    }
                    _ => {}
                }
            } else if !self.alive() {
                let internal = stderr.finish().map_err(|e| e.to_string())?;
                return Err(format!(
                    "{} kernel exited before response: {}",
                    self.language.label(),
                    internal.text
                ));
            } else {
                thread::sleep(POLL);
            }
        }
    }

    fn next_frame(&mut self) -> Result<Option<Value>, String> {
        let mut chunk = [0u8; 8192];
        match self.stdout.read_available(&mut chunk) {
            Ok(0) => return Ok(None),
            Ok(count) => {
                self.pending.extend_from_slice(&chunk[..count]);
                if self.pending.len() > FRAME_LIMIT {
                    return Err("kernel protocol frame exceeded 64 KiB".into());
                }
            }
            Err(error) if error.kind() == io::ErrorKind::WouldBlock => {}
            Err(error) => return Err(error.to_string()),
        }
        let Some(end) = self.pending.iter().position(|byte| *byte == b'\n') else {
            return Ok(None);
        };
        let line: Vec<u8> = self.pending.drain(..=end).collect();
        serde_json::from_slice(&line)
            .map(Some)
            .map_err(|error| format!("invalid kernel protocol frame: {error}"))
    }

    fn interrupt(&mut self) {
        let _ = self.process_tree.signal(ProcessSignal::Interrupt);
        thread::sleep(Duration::from_millis(100));
    }
    pub(super) fn terminate(&mut self) {
        let _ = self.process_tree.signal(ProcessSignal::Terminate);
        let until = Instant::now() + Duration::from_millis(300);
        while Instant::now() < until {
            if self.child.try_wait().ok().flatten().is_some() {
                return;
            }
            thread::sleep(POLL);
        }
        let _ = self.process_tree.signal(ProcessSignal::Kill);
        let _ = self.child.wait();
    }
}

pub(super) fn drain_kernel_stderr(
    reader: &mut Box<dyn PipeReader>,
    output: &mut BoundedOutput,
) -> Result<(), String> {
    let mut buffer = [0u8; 8192];
    loop {
        match reader.read_available(&mut buffer) {
            Ok(0) => return Ok(()),
            Ok(count) => output
                .write_chunk(&buffer[..count])
                .map_err(|e| e.to_string())?,
            Err(error) if error.kind() == io::ErrorKind::WouldBlock => return Ok(()),
            Err(error) => return Err(error.to_string()),
        }
    }
}
// Three termination flags and three capture buffers, assembled by the poll
// loop that owns them; this call is the first place they become one value.
#[allow(clippy::too_many_arguments)]
pub(super) fn finish_kernel(
    ok: bool,
    cancelled: bool,
    reset: bool,
    stdout: BoundedOutput,
    stderr: BoundedOutput,
    display: BoundedOutput,
    mime: Option<String>,
    error: Option<String>,
) -> Result<KernelResult, String> {
    Ok(KernelResult {
        ok,
        cancelled,
        reset,
        stdout: stdout.finish().map_err(|e| e.to_string())?,
        stderr: stderr.finish().map_err(|e| e.to_string())?,
        display: display.finish().map_err(|e| e.to_string())?,
        display_mime: mime,
        error,
    })
}
