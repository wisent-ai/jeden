use std::io::{self, Write};

use crossterm::terminal::{disable_raw_mode, enable_raw_mode};

use super::render::{compact_prompt, format_message, slash_hint_panel};
use super::{Message, PromptStatus};

pub(super) mod background;
pub(super) mod loops;

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
/// region (relative moves), prints committed lines (each `\r\n`, scrolling into
/// history), then draws the new live region and parks the cursor after it.
pub(super) fn compose_repl(prev_height: usize, committed: &[String], live: &[String]) -> String {
    let mut out = String::new();
    out.push_str("\x1b[?25l\x1b[?7l"); // hide cursor, autowrap off
    // Move to the top of the current live region and erase it downward.
    if prev_height > 0 {
        if prev_height > 1 {
            out.push_str(&format!("\x1b[{}A", prev_height - 1));
        }
        out.push('\r');
        out.push_str("\x1b[0J");
    }
    // Committed lines flow into scrollback; CRLF scrolls the terminal as needed.
    for line in committed {
        out.push_str(line);
        out.push_str("\r\n");
    }
    // Live region: drawn in place, no trailing newline after the last line.
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

/// The bottom live region: optional slash-suggestion panel + the prompt line(s).
pub(super) fn live_lines(status: &PromptStatus, input: &str, slash_selection: usize, columns: usize, color: bool) -> Vec<String> {
    let width = columns.min(120).max(50);
    let mut lines = Vec::new();
    lines.extend(slash_hint_panel(input, width, color, slash_selection));
    lines.extend(compact_prompt(width, status, input, false, color));
    lines
        .into_iter()
        .flat_map(|line| line.split('\n').map(str::to_string).collect::<Vec<_>>())
        .collect()
}

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

pub(super) fn push_turn_result(messages: &mut Vec<Message>, prompt: &str, result: Result<String, String>) {
    match result {
        Ok(text) => messages.push(Message::new(if prompt.starts_with('/') { "system" } else { "assistant" }, text.trim().to_string())),
        Err(error) => messages.push(Message::new("error", error)),
    }
}
