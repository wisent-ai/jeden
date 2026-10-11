//! A chat's memory that never ends: every message kept word for word in an
//! append-only log, a tree of summaries over it built in the background by a
//! cheap model through Brama, and a view of constant size that each turn is
//! given instead of the whole history, with the tree reachable by zoom and
//! by date. The view is kept beside the log, so a resumed session continues
//! from it. Compaction, which replaced the history and lost what the operator
//! said, is not used while the chat memory is on.
//!
//! The operator turns it on per workspace or user with `chatMemory` in
//! Jeden's configuration: `summaryModel` (the Brama route or alias of the
//! cheap model) and `viewBytes` (how many bytes the view may take). A
//! summary's size is the operator's own number, declared beside this file.
//! Everything lives under the session directory in `chat-memory/`.

mod log;
pub mod session;
mod tree;
mod view;

use serde::Deserialize;
use serde_json::Value;
use std::path::{Path, PathBuf};
use std::sync::LazyLock;

pub use log::{Entry, Log};
pub use tree::{Node, Summarize, Tree};
pub use view::View;

#[derive(Deserialize)]
struct Declared {
    value: usize,
}

#[derive(Deserialize)]
struct Limits {
    summary_bytes_max: Declared,
}

static LIMITS: LazyLock<Limits> = LazyLock::new(|| {
    serde_json::from_str(include_str!("numeric-provenance.json"))
        .expect("numeric-provenance.json declares the operator's summary size")
});

/// The most bytes one summary of the tree may take: the operator's number.
pub fn summary_bytes() -> usize {
    LIMITS.summary_bytes_max.value
}

/// What the operator set the chat memory to.
#[derive(Clone, Debug)]
pub struct Settings {
    /// The Brama route or alias the summaries are written with.
    pub summary_model: String,
    /// How many bytes the view a turn is given may take.
    pub view_bytes: usize,
}

/// The chat memory's settings in `cwd`'s configuration: none when the
/// configuration names no `chatMemory`, which leaves it off. A section that
/// leaves out a setting, or states one that is not usable, is refused naming
/// it, never filled in.
pub fn settings(cwd: &Path) -> Result<Option<Settings>, String> {
    let merged = crate::cli::config::merged_config_value(cwd);
    let section = match merged.get("chatMemory") {
        None | Some(Value::Null) => return Ok(None),
        Some(section) => section,
    };
    let summary_model = section
        .get("summaryModel")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|model| !model.is_empty())
        .ok_or("chatMemory.summaryModel is required: the Brama route or alias of the cheap model the summary tree is written with")?
        .to_string();
    let view_bytes = section
        .get("viewBytes")
        .and_then(Value::as_u64)
        .and_then(|bytes| usize::try_from(bytes).ok())
        .and_then(std::num::NonZeroUsize::new)
        .ok_or("chatMemory.viewBytes is required: a positive whole number of bytes the view a turn is given may take")?
        .get();
    if view_bytes < summary_bytes() {
        return Err(format!(
            "chatMemory.viewBytes {view_bytes} is smaller than one summary of the tree ({} bytes), so no view could hold a node",
            summary_bytes()
        ));
    }
    Ok(Some(Settings {
        summary_model,
        view_bytes,
    }))
}

/// Where a session keeps its chat memory.
pub fn dir(session_dir: &Path) -> PathBuf {
    session_dir.join("chat-memory")
}
