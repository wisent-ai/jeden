//! The interactive prompt loop: draw, read one event, act on it.

use std::io::{self, IsTerminal, Write};
use std::path::Path;
use std::time::Duration;

use crossterm::event::{self, Event, KeyCode, KeyEventKind, KeyModifiers};


use crate::tui::{
    stdout_supports_color, AttachmentTray, CommandOutcome, ConfirmEvent, ConfirmState,
    EditorState, FollowUpQueue, Message, PickerEvent, PickerSpec, PickerState, PromptStatus,
    RegistryUiRuntime, TurnCtx, TurnKind,
};

use super::{message_block, RawModeGuard, ReplRenderer};

mod keys;
mod live;
mod plain;
mod session;

use keys::handle_editing_key;
use live::{draw_welcome, editor_live_lines};
use plain::{attachment_command, old_read_line_loop, terminal_dimensions, BracketedPasteGuard};
use session::run_turn_chain;

pub fn run_basic_loop<S, C, H>(
    mut status_provider: S,
    mut classify: C,
    handler: H,
    initial_picker: Option<PickerSpec>,
) -> io::Result<()>
where
    S: FnMut() -> PromptStatus,
    C: FnMut(&str) -> TurnKind,
    H: Fn(&str, &TurnCtx) -> Result<CommandOutcome, String> + Sync,
{
    if !io::stdin().is_terminal() || !io::stdout().is_terminal() {
        return old_read_line_loop(status_provider, classify, handler);
    }

    let _raw = RawModeGuard::enter()?;
    let _paste = BracketedPasteGuard::enter()?;
    let mut messages: Vec<Message> = Vec::new();
    let mut committed = 0usize;
    let mut editor = EditorState::default();
    let mut slash_selection = 0usize;
    let mut needs_render = false;
    let mut renderer = ReplRenderer::new();
    let mut picker = initial_picker.map(PickerState::new);
    let mut confirm: Option<ConfirmState> = None;
    // Output of the last slash command. It lives here rather than in the
    // transcript so the next command replaces it instead of stacking.
    let mut view: Option<Message> = None;
    let mut submission_from_view = false;
    let mut attachments = AttachmentTray::default();
    let mut follow_ups = FollowUpQueue::default();
    let runtime = RegistryUiRuntime;

    draw_welcome(
        &status_provider(),
        &editor,
        &attachments,
        picker.as_ref(),
        &mut renderer,
    )?;

    loop {
        if needs_render {
            let status = status_provider();
            let (columns, rows) = terminal_dimensions();
            let color = stdout_supports_color();
            let mut new_blocks = Vec::new();
            for message in &messages[committed..] {
                new_blocks.extend(message_block(message, columns.min(112), color));
            }
            committed = messages.len();
            let (live, cursor_rows_below) = editor_live_lines(
                &status,
                &editor,
                &attachments,
                slash_selection,
                picker.as_ref(),
                confirm.as_ref(),
                view.as_ref(),
                columns,
                rows,
                color,
            );
            renderer.flush_with_cursor(&new_blocks, &live, cursor_rows_below)?;
            needs_render = false;
        }

        if !event::poll(Duration::from_millis(250))? {
            continue;
        }
        let event = event::read()?;
        match event {
            Event::Resize(_, _) => {
                needs_render = true;
                continue;
            }
            Event::Paste(text) => {
                if picker.is_none() && confirm.is_none() {
                    editor.paste(&text);
                    slash_selection = 0;
                    if let Some(error) = editor.take_error() {
                        messages.push(Message::new("error", error.to_string()));
                    }
                    needs_render = true;
                }
                continue;
            }
            Event::Key(key) if matches!(key.kind, KeyEventKind::Press | KeyEventKind::Repeat) => {
                needs_render = true;
                let ctrl_c =
                    key.code == KeyCode::Char('c') && key.modifiers.contains(KeyModifiers::CONTROL);
                let ctrl_d = key.code == KeyCode::Char('d')
                    && key.modifiers.contains(KeyModifiers::CONTROL)
                    && editor.is_empty();
                if ctrl_c || ctrl_d {
                    break;
                }

                if let Some(active_confirm) = confirm.as_mut() {
                    match active_confirm.handle_key(key) {
                        ConfirmEvent::Pending => continue,
                        ConfirmEvent::Cancelled => {
                            confirm = None;
                            continue;
                        }
                        ConfirmEvent::Submit(command) => {
                            confirm = None;
                            picker = None;
                            editor.set_text(command);
                            submission_from_view = true;
                        }
                    }
                }
                if let Some(active_picker) = picker.as_mut() {
                    match active_picker.handle_key(key) {
                        PickerEvent::Pending => continue,
                        PickerEvent::Cancelled => {
                            picker = None;
                            continue;
                        }
                        PickerEvent::Submit(command) => {
                            picker = None;
                            editor.set_text(command);
                            submission_from_view = true;
                        }
                        PickerEvent::Prefill(command) => {
                            picker = None;
                            editor.set_text(command);
                            submission_from_view = false;
                            continue;
                        }
                        PickerEvent::Confirm {
                            label,
                            detail,
                            command,
                        } => {
                            confirm = Some(ConfirmState::new(
                                label,
                                detail,
                                command,
                                active_picker.spec.lang.clone(),
                            ));
                            continue;
                        }
                    }
                }

                // Esc closes the open command view first — the same key that
                // dismisses a picker, so both kinds of view behave alike.
                if key.code == KeyCode::Esc && view.is_some() {
                    view = None;
                    continue;
                }

                let submit = handle_editing_key(
                    key,
                    &mut editor,
                    &mut slash_selection,
                    &mut messages,
                    &mut attachments,
                    &mut renderer,
                    &runtime,
                    &mut status_provider,
                )?;
                if let Some(error) = editor.take_error() {
                    messages.push(Message::new("error", error.to_string()));
                    continue;
                }
                if !submit {
                    continue;
                }

                if editor.text().trim().is_empty() {
                    continue;
                }
                let submitted = editor.take();
                if let Some(result) = attachment_command(
                    &submitted,
                    Path::new(&status_provider().cwd),
                    &mut attachments,
                ) {
                    submission_from_view = false;
                    slash_selection = 0;
                    editor.push_history(submitted.clone());
                    messages.push(Message::new("user", submitted));
                    match result {
                        Ok(text) => messages.push(Message::new("system", text)),
                        Err(error) => messages.push(Message::new("error", error)),
                    }
                    needs_render = true;
                    continue;
                }
                let active_prompt = submitted;
                let active_attachments = attachments.take_all();
                let active_from_view = submission_from_view;
                submission_from_view = false;
                slash_selection = 0;
                if matches!(active_prompt.trim(), "/exit" | "/quit") {
                    break;
                }
                editor.push_history(active_prompt.clone());
                // A command replaces the open view; only conversation is
                // echoed into the scrollback, so running ten commands leaves
                // one panel on screen instead of twenty stale frames.
                let is_command = active_prompt.trim_start().starts_with('/');
                view = None;
                if !is_command {
                    messages.push(Message::new("user", active_prompt.clone()));
                }

                {
                    let (columns, _) = terminal_dimensions();
                    let color = stdout_supports_color();
                    let mut blocks = Vec::new();
                    for message in &messages[committed..] {
                        blocks.extend(message_block(message, columns.min(112), color));
                    }
                    committed = messages.len();
                    renderer.flush(&blocks, &[])?;
                }

                if run_turn_chain(
                    active_prompt,
                    active_attachments,
                    active_from_view,
                    &mut status_provider,
                    &mut classify,
                    &handler,
                    &mut renderer,
                    &mut messages,
                    &mut committed,
                    &mut editor,
                    &mut follow_ups,
                    &mut picker,
                    &mut view,
                    &runtime,
                )? {
                    break;
                }
                needs_render = true;
            }
            _ => continue,
        }
    }

    let (columns, _) = terminal_dimensions();
    let color = stdout_supports_color();
    let mut final_blocks = Vec::new();
    for message in &messages[committed..] {
        final_blocks.extend(message_block(message, columns.min(112), color));
    }
    renderer.flush(&final_blocks, &[])?;
    let mut stdout = io::stdout();
    stdout.write_all(b"\r\n")?;
    stdout.flush()
}
