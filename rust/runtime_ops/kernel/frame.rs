//! One protocol frame a kernel writes on stdout, read for what it states.
//!
//! The bootstraps (`bootstrap.rs`) write exactly two kinds of frame for an
//! evaluation: a `chunk` of one stream's output and the `done` that ends it.
//! A frame of this evaluation that is neither, or that leaves out what its
//! kind carries, is a broken protocol and is refused quoting the frame: read
//! as a frame to skip, it left the evaluation waiting for a `done` that never
//! came, and read with defaults it reported output and verdicts the kernel
//! never stated.

use serde::Deserialize;
use serde_json::Value;

/// The output stream a chunk belongs to.
#[derive(Debug, Clone, Copy, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub(super) enum Stream {
    Stdout,
    Stderr,
    Display,
}

#[derive(Debug, Deserialize)]
#[serde(tag = "type", rename_all = "lowercase")]
pub(super) enum Frame<'a> {
    /// Part of one stream's output; `mime` names a display's media type.
    Chunk {
        stream: Stream,
        #[serde(borrow)]
        data: &'a str,
        #[serde(borrow)]
        mime: Option<&'a str>,
    },
    /// The evaluation ended: whether it succeeded, and its error when not.
    Done {
        ok: bool,
        #[serde(borrow)]
        error: Option<&'a str>,
    },
}

/// The evaluation id a frame answers, or `None` when it names none. Ids are
/// opaque text the bootstraps echo back unread.
pub(super) fn id_of(frame: &Value) -> Option<&str> {
    frame.get("id").and_then(Value::as_str)
}

/// `frame` as the kind it states, or the refusal quoting it.
pub(super) fn read(frame: &Value) -> Result<Frame<'_>, String> {
    Frame::deserialize(frame)
        .map_err(|error| format!("invalid kernel protocol frame {frame}: {error}"))
}
