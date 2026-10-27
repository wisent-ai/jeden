//! Turning the terminal state into the lines a frame is made of.

use super::text::{paint, sanitize_terminal_text, wrap_line};
use super::{
    AttachmentTray, EditorState, FollowUpQueue, FrameOptions, Message, Role, ASSISTANT_TITLE,
};

mod panels;
mod prompt;
// qr.rs lives here, beside the panels that draw it; the crate reaches it as
// `tui::qr`.
pub mod qr;

use crate::tui::render::panels::boxes::{input_prefix_width, INPUT_PREFIX};
use std::num::NonZeroUsize;
use crate::tui::text::clamp_visible;
use crate::tui::text::visible_len;
pub(crate) use panels::{boxed, boxed_split, welcome_panel};
pub(super) use panels::{complete_slash_input, slash_hint_panel, slash_matches};
pub(crate) use prompt::compact_prompt;

fn role_color(role: Role) -> &'static str {
    match role {
        Role::Assistant => "magenta",
        Role::Error => "red",
        Role::System => "yellow",
        // Machinery the communication mode chose to show sits below the
        // conversation it explains.
        Role::Tool => "dim",
        Role::Reasoning => "dim",
        Role::User => "cyan",
        Role::Status => "cyan",
    }
}

pub(super) fn format_message(message: &Message, width: usize, color: bool) -> Vec<String> {
    let name = message.role.name();
    let title = sanitize_terminal_text(match message.role {
        Role::Assistant => ASSISTANT_TITLE,
        _ => name.as_str(),
    });
    let safe = sanitize_terminal_text(&message.text);
    let rows = safe.split('\n').map(str::to_string).collect::<Vec<_>>();
    boxed(&title, &rows, width.max(usize::from(crate::tally::STEP)), color)
        .into_iter()
        .map(|line| paint(&line, role_color(message.role), color))
        .collect()
}

pub(super) fn place_editor_cursor(
    lines: &mut [String],
    input: &str,
    cursor: usize,
    width: usize,
    trailing_rows: usize,
) -> usize {
    let one = usize::from(crate::tally::STEP);
    let rendered_input_rows = lines.len().saturating_sub(trailing_rows + one);
    let prefix_width = input_prefix_width(width.max(one));
    let content_width = width.saturating_sub(prefix_width).max(one);
    let safe_prefix = sanitize_terminal_text(&input[..cursor.min(input.len())]);
    // Rows and columns count from the input's first row and column.
    let mut cursor_row = crate::tally::ORIGIN;
    let mut cursor_column = crate::tally::ORIGIN;
    for (index, logical) in safe_prefix.split('\n').enumerate() {
        // Every logical line after the first starts a row of its own.
        if index != crate::tally::ORIGIN {
            cursor_row += one;
        }
        match NonZeroUsize::new(visible_len(logical)) {
            None => cursor_column = crate::tally::ORIGIN,
            Some(logical_width) => {
                let last = logical_width.get() - one;
                cursor_row += last / content_width;
                cursor_column = last % content_width + one;
            }
        }
    }
    let up = trailing_rows + rendered_input_rows.saturating_sub(cursor_row + one);
    let Some(last) = lines.last_mut() else {
        return crate::tally::ORIGIN;
    };
    if let Some(up) = NonZeroUsize::new(up) {
        last.push_str(&format!("\x1b[{up}A"));
    }
    last.push('\r');
    if let Some(column) = NonZeroUsize::new(cursor_column + prefix_width) {
        last.push_str(&format!("\x1b[{column}C"));
    }
    up
}

pub(super) fn attachment_lines(tray: &AttachmentTray, width: usize, color: bool) -> Vec<String> {
    if tray.items().is_empty() {
        return Vec::new();
    }
    let heading = clamp_visible(
        &format!("Attachments ({})", tray.items().len()),
        width.max(usize::from(crate::tally::STEP)),
    );
    let mut lines = vec![paint(&heading, "bold", color)];
    for attachment in tray.items() {
        lines.push(clamp_visible(
            &format!("  {}", sanitize_terminal_text(&attachment.fallback_label())),
            width.max(usize::from(crate::tally::STEP)),
        ));
    }
    lines
}

pub(super) fn busy_editor_lines(
    editor: &EditorState,
    queue: &FollowUpQueue,
    width: usize,
    color: bool,
) -> Vec<String> {
    let one = usize::from(crate::tally::STEP);
    let width = width.max(one);
    let prefix_width = input_prefix_width(width);
    let content_width = width.saturating_sub(prefix_width).max(one);
    // The first row carries as much of the prefix as fits; the rows after it
    // are indented as far.
    let first_prefix: String = INPUT_PREFIX.chars().take(prefix_width).collect();
    let indent = " ".repeat(prefix_width);
    let label = if queue.is_empty() {
        "Follow-up".to_string()
    } else {
        format!("Follow-up ({} queued)", queue.len())
    };
    let hotkeys = clamp_visible("[Enter] queue  [Ctrl+Enter] steer  [Alt+Up] recall", width);
    let lines_label = clamp_visible(&label, width);
    let mut lines = vec![
        paint(&hotkeys, "dim", color),
        paint(&lines_label, "dim", color),
    ];
    let safe = sanitize_terminal_text(editor.text());
    let mut first = true;
    for logical in safe.split('\n') {
        for part in wrap_line(logical, content_width) {
            let prefix = if first { &first_prefix } else { &indent };
            lines.push(format!("{prefix}{part}"));
            first = false;
        }
    }
    if first {
        lines.push(first_prefix);
    }
    lines
}

pub(super) fn frame_lines(options: &FrameOptions) -> Vec<String> {
    let width = options.columns;
    let prompt = compact_prompt(
        width,
        &options.status,
        &options.input_text,
        options.busy,
        options.color,
    );
    let slash_hints = slash_hint_panel(
        &options.input_text,
        width,
        options.color,
        options.slash_selection,
    );
    // The frame leaves the terminal's last row clear, so a newline written
    // after it never scrolls the frame away.
    let reserved = prompt.len() + slash_hints.len() + usize::from(crate::tally::STEP);
    let available_rows = options.rows.saturating_sub(reserved);
    let message_lines: Vec<String> = options
        .messages
        .iter()
        .flat_map(|message| format_message(message, width, options.color))
        .collect();
    let mut main_lines = if message_lines.is_empty() {
        welcome_panel(
            width,
            &options.status.model,
            &options.status.cwd,
            &options.status.write_status,
            &options.status.command_status,
            options.color,
        )
    } else {
        message_lines
    };
    if main_lines.len() > available_rows {
        main_lines = main_lines.split_off(main_lines.len() - available_rows);
    }
    let mut lines = Vec::new();
    lines.extend(main_lines);
    lines.extend(std::iter::repeat_n(
        String::new(),
        available_rows.saturating_sub(lines.len()),
    ));
    lines.extend(slash_hints);
    lines.extend(prompt);
    // Never let an embedded newline reach the absolute-positioned diff renderer.
    lines
        .into_iter()
        .flat_map(|line| line.split('\n').map(str::to_string).collect::<Vec<_>>())
        .collect()
}

pub fn render_terminal_frame(options: &FrameOptions) -> String {
    let lines = frame_lines(options);
    if options.color {
        format!("\x1b[2J\x1b[H{}", lines.join("\n"))
    } else {
        lines
            .into_iter()
            .map(|line| sanitize_terminal_text(&line))
            .collect::<Vec<_>>()
            .join("\n")
    }
}
