use std::io::{self, Write};

use crossterm::terminal::{disable_raw_mode, enable_raw_mode};

use super::render::format_message;
use super::{CommandOutcome, Message, PickerState};

pub(super) mod background;
pub(crate) mod external_editor;
pub(super) mod loops;
pub(super) mod questions;

/// Sticky-prompt renderer for native scrollback. Finalized transcript blocks are
/// printed once into the terminal's normal buffer (they scroll into real history
/// and persist); only the bottom "live region" (prompt / spinner / streamed text)
/// is repainted in place. All cursor moves are RELATIVE, so terminal scrolling
/// from committed output never corrupts positioning.
pub(super) struct ReplRenderer {
    live_height: usize,
}

impl ReplRenderer {
    pub(super) fn new() -> Self {
        Self { live_height: 0 }
    }

    pub(super) fn reset(&mut self) {
        self.live_height = 0;
    }

    /// Erase the current live region, print `committed` lines into scrollback,
    /// then repaint `live` at the bottom. One atomic write.
    pub(super) fn flush(&mut self, committed: &[String], live: &[String]) -> io::Result<()> {
        let out = compose_repl(self.live_height, committed, live);
        let mut stdout = io::stdout();
        stdout.write_all(out.as_bytes())?;
        stdout.flush()?;
        self.live_height = live.len();
        Ok(())
    }
}

/// Pure ANSI generator for the sticky-prompt renderer. Erases the previous live
/// region, prints committed lines into scrollback, then bottom-aligns the new
/// live region. Relative moves remain valid when the terminal scrolls.
pub(super) fn compose_repl(prev_height: usize, committed: &[String], live: &[String]) -> String {
    let mut out = String::new();
    out.push_str("\x1b[?25l\x1b[?7l"); // hide cursor, autowrap off
    if prev_height > 0 {
        if prev_height > 1 {
            out.push_str(&format!("\x1b[{}A", prev_height - 1));
        }
        out.push('\r');
        out.push_str("\x1b[0J");
    }
    for line in committed {
        out.push_str(line);
        out.push_str("\r\n");
    }

    // Expanding the live region needs scrollback space. Account for scrolling
    // already caused by committed CRLFs, then create only the missing rows.
    let previous_capacity = prev_height.max(1);
    let automatic_scroll = committed
        .len()
        .saturating_sub(previous_capacity.saturating_sub(1));
    let required_scroll = live
        .len()
        .saturating_add(committed.len())
        .saturating_sub(previous_capacity);
    let additional_scroll = required_scroll.saturating_sub(automatic_scroll);
    if additional_scroll > 0 {
        out.push_str("\x1b[999B\r");
        for _ in 0..additional_scroll {
            out.push_str("\r\n");
        }
    }

    if !live.is_empty() {
        out.push_str("\x1b[999B\r");
        if live.len() > 1 {
            out.push_str(&format!("\x1b[{}A", live.len() - 1));
        }
    }
    for (index, line) in live.iter().enumerate() {
        if index > 0 {
            out.push_str("\r\n");
        }
        out.push_str("\x1b[2K");
        out.push_str(line);
    }
    out.push_str("\x1b[?7h\x1b[?25h"); // autowrap back on, show cursor
    out
}

/// One finalized message rendered as scrollback lines (boxed, newline-split).
pub(super) fn message_block(message: &Message, columns: usize, color: bool) -> Vec<String> {
    let width = columns.min(120).max(50);
    format_message(message, width, color)
        .into_iter()
        .flat_map(|line| line.split('\n').map(str::to_string).collect::<Vec<_>>())
        .collect()
}

/// The bottom live region: an active interactive view above the prompt, or
/// slash suggestions below it.

struct RawModeGuard;

impl RawModeGuard {
    fn enter() -> io::Result<Self> {
        enable_raw_mode()?;
        Ok(Self)
    }
}

impl Drop for RawModeGuard {
    fn drop(&mut self) {
        let _ = disable_raw_mode();
    }
}

pub(super) fn spinner_glyph(frame: usize) -> char {
    const FRAMES: [char; 10] = ['⠋', '⠙', '⠹', '⠸', '⠼', '⠴', '⠦', '⠧', '⠇', '⠏'];
    FRAMES[frame % FRAMES.len()]
}

pub(super) fn apply_turn_result(
    messages: &mut Vec<Message>,
    prompt: &str,
    result: Result<CommandOutcome, String>,
    picker: &mut Option<PickerState>,
) -> bool {
    match result {
        Ok(CommandOutcome::Text(text)) => {
            messages.push(Message::new(
                if prompt.starts_with('/') {
                    "system"
                } else {
                    "assistant"
                },
                text.trim().to_string(),
            ));
            false
        }
        Ok(CommandOutcome::Exit(text)) => {
            messages.push(Message::new("system", text.trim().to_string()));
            true
        }
        Ok(CommandOutcome::Picker(spec)) => {
            *picker = Some(PickerState::new(spec));
            false
        }
        Err(error) => {
            messages.push(Message::new("error", error));
            false
        }
    }
}
