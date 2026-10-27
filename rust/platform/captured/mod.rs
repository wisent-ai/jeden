//! Running a workspace command to its end and capturing what it printed, for
//! every platform's workspace isolation: one copy of the steps, where each
//! platform kept its own and only one of them stopped deadlocking on stdin.
//! Every wait goes through stado_wait, which names it while it lasts.

use std::io::Write;
use std::process::{Command, Stdio};

use super::platform::PlatformError;

/// Run `command`, refusing its failure with what it was doing, its exit
/// status and what it printed on stderr.
pub(crate) fn checked(command: &mut Command, what: &str) -> Result<(), PlatformError> {
    let output = stado_wait::output(command.stdin(Stdio::null()))?;
    if output.status.success() {
        Ok(())
    } else {
        Err(PlatformError::Process(format!(
            "{what} failed with {}: {}",
            output.status,
            String::from_utf8_lossy(&output.stderr).trim()
        )))
    }
}

/// The size of a captured stream, refused when it is beyond `max`.
fn within(stream: &[u8], max: u64) -> Result<u64, PlatformError> {
    let size = u64::try_from(stream.len()).map_err(|error| {
        PlatformError::Process(format!("a captured stream cannot be measured: {error}"))
    })?;
    if size > max {
        return Err(PlatformError::TooLarge { size, limit: max });
    }
    Ok(size)
}

/// Run `command` to its end and answer its stdout, refused beyond `max`.
/// `reports_differences` is for diff(1), which answers found differences
/// with a failing status and no message and keeps stderr for trouble; any
/// other failure is refused with the program, its exit status and its stderr.
pub(crate) fn bounded(
    command: &mut Command,
    max: u64,
    reports_differences: bool,
) -> Result<Vec<u8>, PlatformError> {
    let program = command.get_program().to_string_lossy().into_owned();
    let out = stado_wait::output(command.stdin(Stdio::null()))?;
    within(&out.stdout, max)?;
    let differences = reports_differences && out.stderr.is_empty();
    if !out.status.success() && !differences {
        return Err(PlatformError::Process(format!(
            "{program} failed with {}: {}",
            out.status,
            String::from_utf8_lossy(&out.stderr).trim()
        )));
    }
    Ok(out.stdout)
}

/// Run `command` with `input` on its stdin and answer whether it succeeded,
/// with its stdout and stderr, each refused beyond `max`. The input is
/// written from its own thread while the outputs are read: written first on
/// the calling thread, a child that answers more than a pipe buffer before
/// reading all its input blocked on stdout while Jeden blocked on stdin, and
/// the job hung without a word.
pub(crate) fn bounded_with_stdin(
    command: &mut Command,
    max: u64,
    input: &[u8],
) -> Result<(bool, Vec<u8>, Vec<u8>), PlatformError> {
    let program = command.get_program().to_string_lossy().into_owned();
    let mut child = command
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()?;
    let mut stdin = child
        .stdin
        .take()
        .ok_or_else(|| PlatformError::Process(format!("{program} was started without a stdin")))?;
    let owned = input.to_vec();
    let writer = std::thread::spawn(move || stdin.write_all(&owned));
    let out = stado_wait::child_output(child, format!("{program} reading the workspace snapshot"))?;
    let written = writer.join().map_err(|panic| {
        PlatformError::Process(crate::panicked::refusal(
            &format!("the stdin writer of {program}"),
            &*panic,
        ))
    })?;
    if let Err(error) = written {
        return Err(PlatformError::Process(format!(
            "the workspace snapshot could not be given to {program} ({}): {error}; it said: {}",
            out.status,
            String::from_utf8_lossy(&out.stderr).trim()
        )));
    }
    within(&out.stdout, max)?;
    within(&out.stderr, max)?;
    Ok((out.status.success(), out.stdout, out.stderr))
}
