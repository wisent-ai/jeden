//! The bottom region of the screen that is redrawn as the operator types.
//!
//! Split out of `tui/repl/loops.rs`, which had grown past the module line cap.

use crate::tui::render::{
    attachment_lines, compact_prompt, place_editor_cursor, slash_hint_panel,
};
use crate::tui::view_render::{confirm_panel, picker_panel};
use super::super::ReplRenderer;
use crate::tui::{
    AttachmentTray, ConfirmState, EditorState, Message, PickerState, PromptStatus,
};
use crate::tui::text::sanitize_terminal_text;
use message_block;

/// The bottom live region: an active interactive view above the prompt, or a
/// fixed prompt followed by a shrinking slash-suggestion panel.
// Each parameter is a separate borrow of independent REPL state, so there is no
// owner struct to group them behind without cloning at both call sites.
#[allow(clippy::too_many_arguments)]
pub(super) fn editor_live_lines(
    status: &PromptStatus,
    editor: &EditorState,
    attachments: &AttachmentTray,
    slash_selection: usize,
    picker: Option<&PickerState>,
    confirm: Option<&ConfirmState>,
    view: Option<&Message>,
    columns: usize,
    rows: usize,
    color: bool,
) -> (Vec<String>, usize) {
    let _capabilities = crate::capability::for_cwd(std::path::Path::new(&status.cwd));
    let width = columns.clamp(1, 112);
    let has_interactive_view = picker.is_some() || confirm.is_some() || view.is_some();
    let mut cursor_rows_below = 0;
    let prompt: Vec<String> = compact_prompt(width, status, editor.text(), false, color)
        .into_iter()
        .flat_map(|line| line.split('\n').map(str::to_string).collect::<Vec<_>>())
        .collect();
    let prompt_height = prompt.len();
    // The live region must fit the terminal: the interactive view only gets
    // the rows left after the prompt block (plus a one-line margin), so its
    // title, search line, tab bar and footer never scroll off the top.
    let view_rows = rows.saturating_sub(prompt_height + 1);
    let mut lines = if let Some(confirm) = confirm {
        confirm_panel(confirm, width, color)
    } else if let Some(picker) = picker {
        picker_panel(picker, width, view_rows, color)
    } else if let Some(view) = view {
        // Command output lives here, not in the scrollback: the next command
        // overwrites this block instead of stacking another frame under it.
        let block = message_block(view, width, color);
        block.into_iter().take(view_rows).collect()
    } else {
        attachment_lines(attachments, width, color)
    };
    lines = lines
        .into_iter()
        .flat_map(|line| line.split('\n').map(str::to_string).collect::<Vec<_>>())
        .collect();
    let prompt_start = lines.len();
    lines.extend(prompt);
    if !has_interactive_view {
        let slash_hints: Vec<String> =
            slash_hint_panel(editor.text(), width, color, slash_selection)
                .into_iter()
                .flat_map(|line| line.split('\n').map(str::to_string).collect::<Vec<_>>())
                .collect();
        let reserved_hint_rows = if slash_hints.is_empty() {
            0
        } else {
            slash_hint_panel("/", width, color, 0).len()
        };
        let hint_rows = slash_hints.len();
        lines.extend(slash_hints);
        lines.extend(std::iter::repeat_n(
            String::new(),
            reserved_hint_rows.saturating_sub(hint_rows),
        ));
        let trailing_rows = lines.len().saturating_sub(prompt_start + prompt_height);
        cursor_rows_below = place_editor_cursor(
            &mut lines[prompt_start..],
            editor.text(),
            editor.cursor(),
            width,
            trailing_rows,
        );
    }
    (lines, cursor_rows_below)
}

/// The first frame: the welcome panel above a prompt with nothing typed yet.
pub(super) fn draw_welcome(
    status: &PromptStatus,
    editor: &EditorState,
    attachments: &AttachmentTray,
    picker: Option<&PickerState>,
    renderer: &mut ReplRenderer,
) -> std::io::Result<()> {
    let (columns, rows) = super::plain::terminal_dimensions();
    let color = crate::tui::stdout_supports_color();
    let welcome = crate::tui::render::welcome_panel(
        columns.min(112),
        &status.model,
        &status.cwd,
        &status.write_status,
        &status.command_status,
        color,
    );
    let (live, cursor_rows_below) = editor_live_lines(
        status,
        editor,
        attachments,
        0,
        picker,
        None,
        None,
        columns,
        rows,
        color,
    );
    renderer.flush_with_cursor(&welcome, &live, cursor_rows_below)
}
