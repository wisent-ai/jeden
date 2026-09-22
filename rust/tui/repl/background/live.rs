//! What the screen shows while a background turn is running.
//!
//! Split out of `tui/repl/background.rs`, which had grown past the module line
//! cap.

use crate::tui::Message;
use super::super::{message_block, skeleton_bar};

/// Reasoning is committed the moment the model moves on, so the scrollback
/// keeps the order things happened in: reasoning, tool call, result, answer.
pub(super) fn commit_reasoning(
    reasoning: &mut String,
    blocks: &mut Vec<String>,
    columns: usize,
    color: bool,
) {
    if reasoning.trim().is_empty() {
        reasoning.clear();
        return;
    }
    blocks.extend(message_block(
        &Message::new("reasoning", std::mem::take(reasoning)),
        columns,
        color,
    ));
}

/// The live region for a background turn: reasoning as it arrives, streamed
/// assistant text, then the skeleton and its status line.
pub(super) fn build_live(
    reasoning: &str,
    streamed: &str,
    note: &str,
    frame: usize,
    cancelling: bool,
    columns: usize,
    color: bool,
) -> Vec<String> {
    let mut lines = Vec::new();
    if !reasoning.trim().is_empty() {
        lines.extend(message_block(
            &Message::new("reasoning", reasoning.to_string()),
            columns,
            color,
        ));
    }
    if !streamed.trim().is_empty() {
        lines.extend(message_block(
            &Message::new("assistant", streamed.to_string()),
            columns,
            color,
        ));
    }
    let label = if cancelling {
        format!("{} cancelling…", skeleton_bar(frame))
    } else {
        format!("{} {} · esc to cancel", skeleton_bar(frame), note)
    };
    lines.extend(message_block(
        &Message::new("system", label),
        columns,
        color,
    ));
    lines
}
