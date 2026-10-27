//! What a kernel evaluation reads off its pipes and the result it becomes.
//!
//! Split out of `process.rs`, which had grown past the module line cap.

use super::super::platform::PipeReader;
use super::super::BoundedOutput;
use super::KernelResult;
use std::io;

/// How many bytes one read of a kernel pipe takes: the size POSIX guarantees
/// a pipe writes whole. <https://pubs.opengroup.org/onlinepubs/9799919799/basedefs/limits.h.html>
pub(super) const PIPE_READ_BYTES: usize = libc::PIPE_BUF;

/// Available native stderr, stopping when the evaluation or scope is cancelled.
/// A continuous stderr producer must not keep cancellation outside this loop.
pub(super) fn drain_kernel_stderr(
    reader: &mut Box<dyn PipeReader>,
    output: &mut BoundedOutput,
    mut cancelled: impl FnMut() -> Result<bool, String>,
) -> Result<(), String> {
    let mut buffer = [Default::default(); PIPE_READ_BYTES];
    loop {
        if cancelled()? { return Ok(()); }
        match reader.read_available(&mut buffer) {
            Ok(count) => {
                let read = &buffer[..count];
                if read.is_empty() {
                    return Ok(());
                }
                output.write_chunk(read).map_err(|e| e.to_string())?
            }
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
