//! What each key does at the prompt when no view has claimed it first.
//!
//! Split out of `tui/repl/loops.rs`, which had grown past the module line cap.

use std::io;
use std::path::Path;

use crossterm::event::{
    DisableBracketedPaste, EnableBracketedPaste, KeyCode, KeyEvent, KeyModifiers,
};
use crossterm::terminal::{disable_raw_mode, enable_raw_mode};

use crate::tool_runtime::runtime_ops::{ArtifactSink, CancellationToken, OperationContext};
use crate::tui::render::{complete_slash_input, slash_matches};
use crate::tui::{
    AttachmentTray, ClipboardContent, EditorAction, EditorState, Message, PromptStatus,
    RegistryUiRuntime, UiRuntimeAdapter,
};

use super::super::external_editor::{external_editor, external_editor_health};
use super::super::ReplRenderer;

/// Returns whether this key asks for the buffer to be submitted.
#[allow(clippy::too_many_arguments)]
pub(super) fn handle_editing_key<S>(
    key: KeyEvent,
    editor: &mut EditorState,
    slash_selection: &mut usize,
    messages: &mut Vec<Message>,
    attachments: &mut AttachmentTray,
    renderer: &mut ReplRenderer,
    runtime: &RegistryUiRuntime,
    status_provider: &mut S,
) -> io::Result<bool>
where
    S: FnMut() -> PromptStatus,
{
    let mut submit = false;
                match key.code {
                    _ if editor.action_for(key) == Some(EditorAction::ExternalEditor)
                        && external_editor_health(Path::new(&status_provider().cwd)).is_ok() =>
                    {
                        let cwd = status_provider().cwd;
                        renderer.flush(&[], &[])?;
                        crossterm::execute!(io::stdout(), DisableBracketedPaste)?;
                        disable_raw_mode()?;
                        let operation = OperationContext::new(
                            CancellationToken::new(),
                            ArtifactSink::new(
                                std::env::temp_dir().join("jeden-external-editor-artifacts"),
                            ),
                        );
                        let result = external_editor(editor, Path::new(&cwd), &operation);
                        enable_raw_mode()?;
                        crossterm::execute!(io::stdout(), EnableBracketedPaste)?;
                        renderer.reset();
                        if let Err(error) = result {
                            messages.push(Message::new("error", error));
                        }
                        *slash_selection = 0;
                    }
                    KeyCode::Esc => {
                        editor.clear();
                        *slash_selection = 0;
                    }
                    KeyCode::Char('v') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                        let cwd = status_provider().cwd;
                        match runtime.read_clipboard(Path::new(&cwd)) {
                            Ok(Some(ClipboardContent::Text(text))) => editor.paste(&text),
                            Ok(Some(content @ ClipboardContent::Bytes { .. })) => {
                                if let Err(error) = attachments.add_clipboard(content) {
                                    messages.push(Message::new("error", error.to_string()));
                                }
                            }
                            Ok(None) => messages.push(Message::new("system", "Clipboard is empty")),
                            Err(error) => messages.push(Message::new("error", error)),
                        }
                    }
                    KeyCode::Backspace if key.modifiers.contains(KeyModifiers::ALT) => {
                        if let Some(id) = attachments.items().last().map(|item| item.id) {
                            attachments.remove(id);
                        }
                    }
                    KeyCode::Enter if key.modifiers.contains(KeyModifiers::ALT) => {
                        editor.apply(EditorAction::InsertNewline);
                        *slash_selection = 0;
                    }
                    KeyCode::Enter | KeyCode::Char('\r') | KeyCode::Char('\n') => submit = true,
                    KeyCode::Char('m') | KeyCode::Char('j')
                        if key.modifiers.contains(KeyModifiers::CONTROL) =>
                    {
                        submit = true
                    }
                    KeyCode::Tab => {
                        if let Some(completed) =
                            complete_slash_input(editor.text(), *slash_selection)
                        {
                            editor.set_text(completed);
                        } else {
                            editor.insert("  ");
                        }
                        *slash_selection = 0;
                    }
                    KeyCode::Right
                        if editor.cursor() == editor.text().len()
                            && !slash_matches(editor.text()).is_empty() =>
                    {
                        if let Some(completed) =
                            complete_slash_input(editor.text(), *slash_selection)
                        {
                            editor.set_text(completed);
                            *slash_selection = 0;
                        }
                    }
                    KeyCode::Up => {
                        let count = slash_matches(editor.text()).len();
                        if count > 0 {
                            *slash_selection = if *slash_selection == 0 {
                                count - 1
                            } else {
                                *slash_selection - 1
                            };
                        } else if editor.text()[..editor.cursor()].contains('\n') {
                            editor.apply(EditorAction::MoveUp);
                        } else {
                            editor.apply(EditorAction::HistoryPrevious);
                        }
                    }
                    KeyCode::Down => {
                        let count = slash_matches(editor.text()).len();
                        if count > 0 {
                            *slash_selection = (*slash_selection + 1) % count;
                        } else if editor.text()[editor.cursor()..].contains('\n') {
                            editor.apply(EditorAction::MoveDown);
                        } else {
                            editor.apply(EditorAction::HistoryNext);
                        }
                    }
                    _ => {
                        if editor.handle_key(key) {
                            *slash_selection = (*slash_selection)
                                .min(slash_matches(editor.text()).len().saturating_sub(1));
                        }
                    }
                }
    Ok(submit)
}
