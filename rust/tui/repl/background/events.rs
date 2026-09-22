//! What the worker thread tells the event loop while a background turn runs,
//! and how each of those is shown.
//!
//! Split out of `tui/repl/background.rs`, which had grown past the module line
//! cap.

use std::io;
use std::sync::mpsc;

use crossterm::event::{self, Event, KeyCode, KeyEventKind, KeyModifiers};

use crate::agent::TraceEvent;
use crate::tui::{default_columns, stdout_supports_color, Message};
use super::super::{message_block, ReplRenderer};

/// Worker→render-loop message during a background turn.
pub(super) enum TurnMsg {
    /// Status line beside the skeleton ("thinking…", "tool: read_file").
    Note(String),
    /// A chunk of live assistant text.
    Delta(String),
    /// A chunk of the model's reasoning; shown live, committed when the
    /// model moves on to a tool call or its answer.
    Reasoning(String),
    /// A tool call or tool result, committed to the scrollback as it happens.
    Trace(Message),
    /// Approval request for a gated tool; the main loop prompts and replies.
    Approve {
        tool: String,
        detail: String,
        reply: mpsc::Sender<bool>,
    },
    AskUser {
        question: String,
        options: Vec<String>,
        reply: mpsc::Sender<Result<String, String>>,
    },
}

const TOOL_INPUT_PREVIEW: usize = 320;
const TOOL_RESULT_PREVIEW: usize = 640;

/// One line of compact JSON, cut at `limit` characters.
fn compact_preview(value: &serde_json::Value, limit: usize) -> String {
    let text = match value {
        serde_json::Value::String(text) => text.clone(),
        other => other.to_string(),
    };
    let text = text.replace(['\r', '\n'], " ");
    match text.char_indices().nth(limit) {
        Some((cut, _)) => format!("{}…", &text[..cut]),
        None => text,
    }
}

/// The scrollback line for one trace event, or none for reasoning, which is
/// accumulated instead.
pub(super) fn trace_message(event: &TraceEvent<'_>) -> Option<Message> {
    match *event {
        TraceEvent::ToolCall { tool, input } => Some(Message::new(
            "tool",
            format!("→ {tool} {}", compact_preview(input, TOOL_INPUT_PREVIEW)),
        )),
        TraceEvent::ToolResult { tool, result } => Some(Message::new(
            "tool",
            format!("← {tool} {}", compact_preview(result, TOOL_RESULT_PREVIEW)),
        )),
        TraceEvent::CompletionState { state } => Some(Message::new(
            "status",
            format!(
                "Tasks: {} — {} verified, {} open",
                state["status"].as_str().unwrap_or("unknown"),
                state["completed"],
                state["open"]
            ),
        )),
        TraceEvent::Message { text } => Some(Message::new("assistant", text)),
        TraceEvent::Reasoning { .. } => None,
    }
}

/// Prompt the user (in the live region) to approve one gated tool. Blocks on a
/// keystroke: `y` allows, anything else (incl. Esc) denies. Returns the choice.
pub(super) fn prompt_tool_approval(
    renderer: &mut ReplRenderer,
    streamed: &str,
    tool: &str,
    detail: &str,
    columns: usize,
    color: bool,
) -> io::Result<bool> {
    let mut lines = Vec::new();
    if !streamed.trim().is_empty() {
        lines.extend(message_block(
            &Message::new("assistant", streamed.to_string()),
            columns,
            color,
        ));
    }
    let ask = if detail.trim().is_empty() {
        format!("Allow tool \"{}\" for this call? [y]es / [n]o", tool)
    } else {
        format!(
            "Allow tool \"{}\" for this call? [y]es / [n]o\nReason: {}",
            tool,
            detail.trim()
        )
    };
    lines.extend(message_block(&Message::new("system", ask), columns, color));
    renderer.flush(&[], &lines)?;
    loop {
        if event::poll(Duration::from_millis(250))? {
            if let Event::Key(key) = event::read()? {
                if !matches!(key.kind, KeyEventKind::Press | KeyEventKind::Repeat) {
                    continue;
                }
                return Ok(matches!(key.code, KeyCode::Char('y') | KeyCode::Char('Y')));
            }
        }
    }
}

/// A question the worker asked: its text, the offered choices, and the channel
/// the event loop answers on.
pub(super) type PendingQuestion = (String, Vec<String>, mpsc::Sender<Result<String, String>>);
