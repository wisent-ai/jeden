//! A chat's memory that never ends, as the operator's OptChat design lays it
//! out: every message logged word for word in `main/YYYY-MM-DD.jsonl`, a
//! binary tree of one-line summaries of at most 512 bytes built from it by a
//! cheap model through Brama in `tree/YYYY-MM-DD.jsonl`, and a view of those
//! lines, 64 to 128 KB, that each turn is given before its new message,
//! saved in `view.json` and never rebuilt. The model opens a line with
//! zoom(id, n), down to the message itself, and asks date(id) for when a
//! message was said. Compaction, which replaced the history and lost what
//! the operator said, is not used while the chat memory is on.
//!
//! The operator turns it on per workspace or user with `chatMemory` in
//! Jeden's configuration, naming `summaryModel`: the Brama route or alias of
//! the cheap model. Every size is the operator's own number, declared
//! beside this file. Everything lives under the session directory in
//! `chat-memory/`.

mod log;
pub mod session;
mod tree;
mod view;

use serde::Deserialize;
use serde_json::Value;
use std::path::{Path, PathBuf};
use std::sync::LazyLock;

pub use log::{Entry, Kind, Log};
pub use tree::{Node, Summarize, Tree};
pub use view::View;

#[derive(Deserialize)]
struct Declared<T> {
    value: T,
}

#[derive(Deserialize)]
struct Limits {
    summary_bytes_max: Declared<usize>,
    view_bytes_high: Declared<usize>,
    view_bytes_low: Declared<usize>,
    echo_chars_max: Declared<usize>,
    summary_attempts_max: Declared<usize>,
    message_level: Declared<u32>,
}

static LIMITS: LazyLock<Limits> = LazyLock::new(|| {
    serde_json::from_str(include_str!("numeric-provenance.json"))
        .expect("numeric-provenance.json declares the operator's chat memory numbers")
});

/// The most bytes one line of the tree may take.
pub fn summary_bytes() -> usize {
    LIMITS.summary_bytes_max.value
}

/// The size past which the view is merged down.
pub fn view_high() -> usize {
    LIMITS.view_bytes_high.value
}

/// The size a batch merges the view down to.
pub fn view_low() -> usize {
    LIMITS.view_bytes_low.value
}

/// How many characters of a tool's output the log keeps, head and tail.
pub fn echo_chars() -> usize {
    LIMITS.echo_chars_max.value
}

/// How many times a compaction is asked for a line that fits.
pub fn summary_attempts() -> usize {
    LIMITS.summary_attempts_max.value
}

/// The level of a message's own node.
pub fn message_level() -> u32 {
    LIMITS.message_level.value
}

/// What the operator set the chat memory to.
#[derive(Clone, Debug)]
pub struct Settings {
    /// The Brama route or alias the summaries are written with.
    pub summary_model: String,
}

/// The chat memory's settings in `cwd`'s configuration: none when the
/// configuration names no `chatMemory`, which leaves it off. A section that
/// leaves out the summary model, or still states a view size the design
/// fixes, is refused naming it, never filled in or ignored.
pub fn settings(cwd: &Path) -> Result<Option<Settings>, String> {
    let merged = crate::cli::config::merged_config_value(cwd);
    let section = match merged.get("chatMemory") {
        None | Some(Value::Null) => return Ok(None),
        Some(section) => section,
    };
    if section.get("viewBytes").is_some() {
        return Err(format!(
            "chatMemory.viewBytes is no longer read: the view grows to {} bytes and is merged down to {} at once, as the design states; remove it",
            view_high(),
            view_low()
        ));
    }
    let summary_model = section
        .get("summaryModel")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|model| !model.is_empty())
        .ok_or("chatMemory.summaryModel is required: the Brama route or alias of the cheap model the summary tree is written with")?
        .to_string();
    Ok(Some(Settings { summary_model }))
}

/// Where a session keeps its chat memory.
pub fn dir(session_dir: &Path) -> PathBuf {
    session_dir.join("chat-memory")
}
