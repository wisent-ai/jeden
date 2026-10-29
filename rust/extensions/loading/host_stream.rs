//! What the wait for the extension host reads: its stdout as progress and
//! its close, an announced cancellation, and the node it runs under.

use std::io::{Read, Write};
use std::process::{ChildStderr, ChildStdout, Command};
use std::sync::mpsc;
use std::thread::JoinHandle;

/// What the wait for the extension host reacts to: stdout progress, stdout
/// closing (the host is done), and an announced cancellation.
pub(super) enum HostEvent {
    Progress(u64, u64),
    StdoutClosed,
    Cancelled,
}

/// The host's stdout as it arrives, kept whole, each chunk announced to the
/// wait as progress.
struct ProgressSink {
    bytes: Vec<u8>,
    events: mpsc::Sender<HostEvent>,
}

impl Write for ProgressSink {
    fn write(&mut self, chunk: &[u8]) -> std::io::Result<usize> {
        self.bytes.extend_from_slice(chunk);
        // A wait that has already ended (a cancelled turn) reads no progress.
        if let Err(mpsc::SendError(_)) = self.events.send(HostEvent::Progress(
            chunk.len() as u64,
            self.bytes.len() as u64,
        )) {
            return Ok(chunk.len());
        }
        Ok(chunk.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

/// Drains stdout whole on its own thread, announcing each chunk and then the
/// close; a read that fails is the thread's answer, never the end of the
/// host's output.
pub(super) fn read_stdout(
    mut stdout: ChildStdout,
    events: mpsc::Sender<HostEvent>,
) -> JoinHandle<std::io::Result<Vec<u8>>> {
    std::thread::spawn(move || {
        let mut sink = ProgressSink {
            bytes: Vec::new(),
            events,
        };
        std::io::copy(&mut stdout, &mut sink)?;
        // A wait that has already ended reads no close either.
        if let Err(mpsc::SendError(_)) = sink.events.send(HostEvent::StdoutClosed) {
            return Ok(sink.bytes);
        }
        Ok(sink.bytes)
    })
}

/// Drains stderr whole on its own thread; a read that fails is its answer.
pub(super) fn read_stderr(mut stderr: ChildStderr) -> JoinHandle<std::io::Result<Vec<u8>>> {
    std::thread::spawn(move || {
        let mut bytes = Vec::new();
        stderr.read_to_end(&mut bytes)?;
        Ok(bytes)
    })
}

/// What one reader thread drained, or why it could not: a panicked thread
/// and a failed read are each refused naming the stream.
pub(super) fn drained(
    reader: JoinHandle<std::io::Result<Vec<u8>>>,
    stream: &str,
) -> Result<Vec<u8>, String> {
    reader
        .join()
        .map_err(|payload| {
            format!(
                "{}, so what it read is lost",
                crate::panicked::refusal(
                    &format!("the extension host's {stream} reader"),
                    &*payload
                )
            )
        })?
        .map_err(|error| format!("the extension host's {stream} could not be read: {error}"))
}

/// Whether `node` strips TypeScript types; a node that cannot be run to ask
/// is refused naming it, never read as one that cannot.
pub(super) fn node_supports_typescript(node: &str) -> Result<bool, String> {
    stado_wait::output(Command::new(node).args(["--experimental-strip-types", "--eval", ""]))
        .map(|output| output.status.success())
        .map_err(|error| {
            format!("{node} cannot be run to ask whether it strips TypeScript types: {error}")
        })
}
