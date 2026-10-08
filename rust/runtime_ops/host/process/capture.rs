//! Reading a child's output as it arrives, bounded, and reporting progress
//! while it does.
//!
//! Split out of `runtime_ops/host/process.rs`, which had grown past the
//! module line cap.

use super::super::super::{BoundedOutput, OperationContext, OperationProgress, OutputCapture};
use super::ProcessEvent;
use std::io::Read;
use std::sync::mpsc::{Receiver, Sender};

pub(super) fn capture_stream(
    stream: &'static str,
    mut reader: impl Read,
    limits: Option<super::OutputLimits>,
    artifacts: super::ArtifactSink,
    events: Sender<ProcessEvent>,
) -> Result<OutputCapture, String> {
    let mut output = BoundedOutput::new(stream, limits, artifacts);
    let mut buffer = [0u8; 8192];
    let mut total = 0u64;
    loop {
        let count = reader
            .read(&mut buffer)
            .map_err(|error| error.to_string())?;
        if count == 0 {
            break;
        }
        output
            .write_chunk(&buffer[..count])
            .map_err(|error| format!("failed capturing {stream}: {error}"))?;
        total = total.saturating_add(count as u64);
        let _ = events.send(ProcessEvent::Progress(OperationProgress {
            stream,
            bytes: count as u64,
            total_bytes: total,
        }));
    }
    output.finish().map_err(|error| error.to_string())
}

/// Reports the progress still queued once the process has been waited for.
pub(super) fn drain_progress(context: &OperationContext<'_>, events: &Receiver<ProcessEvent>) {
    while let Ok(event) = events.try_recv() {
        if let ProcessEvent::Progress(progress) = event {
            context.progress(progress);
        }
    }
}
