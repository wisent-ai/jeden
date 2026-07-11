use std::io::{self, Write};

use crossterm::terminal::{disable_raw_mode, enable_raw_mode};

use super::render::{compact_prompt, format_message, slash_hint_panel};
use super::view_render::{confirm_panel, picker_panel};
use super::{CommandOutcome, ConfirmState, Message, PickerState, PromptStatus};

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

/// The bottom live region: an active interactive view or slash suggestions,
/// followed by the persistent prompt.
pub(super) fn live_lines(
    status: &PromptStatus,
    input: &str,
    slash_selection: usize,
    picker: Option<&PickerState>,
    confirm: Option<&ConfirmState>,
    columns: usize,
    rows: usize,
    color: bool,
) -> Vec<String> {
    let width = columns.min(120).max(50);
    let mut lines = Vec::new();
    if let Some(confirm) = confirm {
        lines.extend(confirm_panel(confirm, width, color));
    } else if let Some(picker) = picker {
        lines.extend(picker_panel(picker, width, rows, color));
    } else {
        lines.extend(slash_hint_panel(input, width, color, slash_selection));
    }
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

pub(super) fn apply_turn_result(
    messages: &mut Vec<Message>,
    prompt: &str,
    result: Result<CommandOutcome, String>,
    picker: &mut Option<PickerState>,
) {
    match result {
        Ok(CommandOutcome::Text(text)) => messages.push(Message::new(
            if prompt.starts_with('/') {
                "system"
            } else {
                "assistant"
            },
            text.trim().to_string(),
        )),
        Ok(CommandOutcome::Picker(spec)) => *picker = Some(PickerState::new(spec)),
        Err(error) => messages.push(Message::new("error", error)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn native_tui_repl_resize_repaints_only_live_region_and_preserves_scrollback_order() {
        for (previous_height, expected_move) in [
            (1, None),
            (3, Some("\x1b[2A")),
            (7, Some("\x1b[6A")),
        ] {
            let rendered = compose_repl(
                previous_height,
                &["committed one".into(), "committed two".into()],
                &["resized live".into()],
            );

            if let Some(expected_move) = expected_move {
                assert!(rendered.contains(expected_move), "height: {previous_height}");
            }
            assert_eq!(rendered.matches("committed one\r\n").count(), 1);
            assert_eq!(rendered.matches("committed two\r\n").count(), 1);
            let erase = rendered.find("\x1b[0J").expect("old live region is erased");
            let first = rendered.find("committed one\r\n").expect("first committed line");
            let second = rendered.find("committed two\r\n").expect("second committed line");
            let live = rendered.find("resized live").expect("new live line");
            assert!(erase < first && first < second && second < live);
            assert!(!rendered.contains("\x1b[H"), "must not address an absolute screen row");
        }
    }

    #[test]
    fn native_tui_non_color_projection_contains_no_terminal_escape_bytes() {
        let status = PromptStatus {
            cwd: "/tmp/project".into(),
            write_status: "allowed".into(),
            command_status: "ask".into(),
            model: "local".into(),
            service_tier: String::new(),
            branch: Some("main".into()),
            dirty_count: 1,
            context_percent: None,
            context_limit: None,
            cost: None,
        };
        let lines = live_lines(&status, "literal \x1b[31m input", 0, None, None, 80, 24, false);
        let message = message_block(&Message::new("assistant", "answer \x1b]0;title\x07 safe"), 80, false);
        let projection = lines.into_iter().chain(message).collect::<Vec<_>>().join("\n");

        assert!(!projection.as_bytes().contains(&0x1b));
        assert!(projection.contains("literal  input"));
        assert!(projection.contains("answer  safe"));
    }
}
