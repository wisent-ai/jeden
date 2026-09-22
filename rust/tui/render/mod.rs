//! Turning the terminal state into the lines a frame is made of.

use std::io::{self, IsTerminal};

use super::text::{paint, sanitize_terminal_text, wrap_line};
use super::{
    AttachmentTray, EditorState, FollowUpQueue, FrameOptions, Message, ASSISTANT_TITLE,
};

mod panels;
mod prompt;
// ca38dda moved qr.rs here from the crate root without declaring it; the
// crate reaches it as `tui::qr`.
pub mod qr;

pub(crate) use panels::{boxed, boxed_split, welcome_panel};
pub(super) use panels::{complete_slash_input, slash_hint_panel, slash_matches};
pub(crate) use prompt::compact_prompt;
use crate::tui::render::panels::boxes::input_prefix_width;
use crate::tui::text::clamp_visible;
use crate::tui::text::visible_len;

fn role_color(role: &str) -> &'static str {
    match role {
        "assistant" => "magenta",
        "error" => "red",
        "system" => "yellow",
        // Machinery the communication mode chose to show sits below the
        // conversation it explains.
        "tool" | "reasoning" => "dim",
        _ => "cyan",
    }
}

pub(super) fn format_message(message: &Message, width: usize, color: bool) -> Vec<String> {
    let width = if io::stdout().is_terminal() {
        crossterm::terminal::size()
            .map(|(columns, _)| width.min(usize::from(columns)).max(1))
            .unwrap_or(width.max(1))
    } else {
        width.max(1)
    };
    let title = sanitize_terminal_text(if message.role == "assistant" {
        ASSISTANT_TITLE
    } else {
        message.role.as_str()
    });
    let safe = sanitize_terminal_text(&message.text);
    let rows = safe.split('\n').map(str::to_string).collect::<Vec<_>>();
    boxed(&title, &rows, width.max(1), color)
        .into_iter()
        .map(|line| paint(&line, role_color(&message.role), color))
        .collect()
}

pub(super) fn place_editor_cursor(
    lines: &mut [String],
    input: &str,
    cursor: usize,
    width: usize,
    trailing_rows: usize,
) -> usize {
    let rendered_input_rows = lines.len().saturating_sub(trailing_rows + 1);
    let prefix_width = input_prefix_width(width.max(1));
    let content_width = width.saturating_sub(prefix_width).max(1);
    let safe_prefix = sanitize_terminal_text(&input[..cursor.min(input.len())]);
    let mut cursor_row = 0usize;
    let mut cursor_column = 0usize;
    for logical in safe_prefix.split('\n') {
        if cursor_row > 0 || cursor_column > 0 {
            cursor_row += 1;
        }
        let logical_width = visible_len(logical);
        if logical_width == 0 {
            cursor_column = 0;
        } else {
            cursor_row += (logical_width - 1) / content_width;
            cursor_column = ((logical_width - 1) % content_width) + 1;
        }
    }
    let up = trailing_rows + rendered_input_rows.saturating_sub(cursor_row + 1);
    let Some(last) = lines.last_mut() else {
        return 0;
    };
    if up > 0 {
        last.push_str(&format!("\x1b[{up}A"));
    }
    last.push('\r');
    let column = cursor_column + prefix_width;
    if column > 0 {
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
        width.max(1),
    );
    let mut lines = vec![paint(&heading, "bold", color)];
    for attachment in tray.items() {
        lines.push(clamp_visible(
            &format!("  {}", sanitize_terminal_text(&attachment.fallback_label())),
            width.max(1),
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
    let width = width.max(1);
    let prefix_width = input_prefix_width(width);
    let content_width = width.saturating_sub(prefix_width).max(1);
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
            let prefix = if first && prefix_width == 2 {
                "> "
            } else if first && prefix_width == 1 {
                ">"
            } else if first {
                ""
            } else if prefix_width == 2 {
                "  "
            } else if prefix_width == 1 {
                " "
            } else {
                ""
            };
            lines.push(format!("{prefix}{part}"));
            first = false;
        }
    }
    if first {
        lines.push(if prefix_width == 2 {
            "> ".into()
        } else if prefix_width == 1 {
            ">".into()
        } else {
            String::new()
        });
    }
    lines
}

pub(super) fn frame_lines(options: &FrameOptions) -> Vec<String> {
    let width = options.columns.clamp(1, 112);
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
    let reserved = prompt.len() + slash_hints.len() + 1;
    let available_rows = options.rows.saturating_sub(reserved).max(4);
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
