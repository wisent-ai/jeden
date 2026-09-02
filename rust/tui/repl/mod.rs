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
/// and persist); only the bottom "live region" (prompt / skeleton / streamed text)
/// is repainted in place. All cursor moves are RELATIVE, so terminal scrolling
/// from committed output never corrupts positioning.
pub(super) struct ReplRenderer {
    live_height: usize,
    cursor_rows_below: usize,
}

impl ReplRenderer {
    pub(super) fn new() -> Self {
        Self {
            live_height: 0,
            cursor_rows_below: 0,
        }
    }

    pub(super) fn reset(&mut self) {
        self.live_height = 0;
        self.cursor_rows_below = 0;
    }

    /// Erase the current live region, print `committed` lines into scrollback,
    /// then repaint `live` from the same anchor. One atomic write.
    pub(super) fn flush(&mut self, committed: &[String], live: &[String]) -> io::Result<()> {
        self.flush_with_cursor(committed, live, 0)
    }

    pub(super) fn flush_with_cursor(
        &mut self,
        committed: &[String],
        live: &[String],
        cursor_rows_below: usize,
    ) -> io::Result<()> {
        let out = compose_repl(self.live_height, self.cursor_rows_below, committed, live);
        let mut stdout = io::stdout();
        stdout.write_all(out.as_bytes())?;
        stdout.flush()?;
        self.live_height = live.len();
        self.cursor_rows_below = cursor_rows_below.min(live.len().saturating_sub(1));
        Ok(())
    }
}

/// Pure ANSI generator for the sticky-prompt renderer. It returns from the
/// editor cursor to the previous live-region end, erases that region, and
/// repaints from the same anchor without jumping to the terminal bottom.
pub(super) fn compose_repl(
    prev_height: usize,
    prev_cursor_rows_below: usize,
    committed: &[String],
    live: &[String],
) -> String {
    let mut out = String::new();
    out.push_str("\x1b[?25l\x1b[?7l"); // hide cursor, autowrap off
    if prev_height > 0 {
        if prev_cursor_rows_below > 0 {
            out.push_str(&format!("\x1b[{}B", prev_cursor_rows_below));
        }
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
pub(crate) fn message_block(message: &Message, columns: usize, color: bool) -> Vec<String> {
    let width = columns.min(120).max(50);
    format_message(message, width, color)
        .into_iter()
        .flat_map(|line| line.split('\n').map(str::to_string).collect::<Vec<_>>())
        .collect()
}

/// The bottom live region: an active interactive view above the prompt, or a
/// fixed prompt followed by a shrinking slash-suggestion panel.

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

/// The terminal's skeleton: a block standing where the answer will land, with a
/// highlight sweeping across it.
///
/// A rotating glyph says only that something is happening. This says the same
/// thing in the shape of the text that is coming, which is the treatment every
/// other Wisent surface gives a wait. The sweep runs out and back so a wrapping
/// frame counter never makes the highlight jump.
pub(super) fn skeleton_bar(frame: usize) -> String {
    const WIDTH: usize = 8;
    let step = frame % (WIDTH * 2 - 2);
    let lit = if step < WIDTH {
        step
    } else {
        WIDTH * 2 - 2 - step
    };
    (0..WIDTH)
        .map(|cell| if cell == lit { '▓' } else { '░' })
        .collect()
}

pub(super) fn apply_turn_result(
    messages: &mut Vec<Message>,
    prompt: &str,
    result: Result<CommandOutcome, String>,
    picker: &mut Option<PickerState>,
    view: &mut Option<Message>,
) -> bool {
    // A slash command is not conversation: its output belongs in the live
    // region, where the next command replaces it, not in the scrollback.
    let is_command = prompt.trim_start().starts_with('/');
    match result {
        Ok(CommandOutcome::Text(text)) => {
            let trimmed = prompt.trim_start();
            let role = if trimmed.starts_with('!') || trimmed.starts_with('$') {
                // Local `!`/`$` escapes render as tool-style result blocks;
                // they never produce a model turn.
                "tool"
            } else if is_command {
                "system"
            } else {
                "assistant"
            };
            if is_command {
                *view = Some(Message::view(role, text.trim().to_string()));
            } else {
                messages.push(Message::new(role, text.trim().to_string()));
            }
            false
        }
        Ok(CommandOutcome::Exit(text)) => {
            messages.push(Message::new("system", text.trim().to_string()));
            true
        }
        Ok(CommandOutcome::Picker(spec)) => {
            *view = None;
            *picker = Some(PickerState::new(spec));
            false
        }
        Err(error) => {
            if is_command {
                *view = Some(Message::view("error", error));
            } else {
                messages.push(Message::new("error", error));
            }
            false
        }
    }
}
