use std::io::{self, IsTerminal, Write};
use std::path::Path;
use std::sync::atomic::AtomicBool;
use std::sync::Arc;
use std::time::Duration;

use crossterm::event::{
    self, DisableBracketedPaste, EnableBracketedPaste, Event, KeyCode, KeyEventKind, KeyModifiers,
};
use crossterm::terminal::{self, disable_raw_mode, enable_raw_mode};

use crate::tool_runtime::runtime_ops::{ArtifactSink, CancellationToken, OperationContext};

use super::super::render::{
    attachment_lines, compact_prompt, complete_slash_input, place_editor_cursor, slash_hint_panel,
    slash_matches, welcome_panel,
};
use super::super::text::sanitize_terminal_text;
use super::super::view_render::{confirm_panel, picker_panel};
use super::super::{
    stdout_supports_color, AttachmentTray, ClipboardContent, CommandOutcome, ConfirmEvent,
    ConfirmState, EditorAction, EditorState, FollowUpQueue, Message, PickerEvent, PickerState,
    PromptStatus, RegistryUiRuntime, TurnCtx, TurnKind, UiFeature, UiRuntimeAdapter,
};
use super::background::run_background_turn;
use super::external_editor::{external_editor, external_editor_health};
use super::{apply_turn_result, message_block, RawModeGuard, ReplRenderer};

fn old_read_line_loop<S, C, H>(
    mut _status_provider: S,
    mut _classify: C,
    handler: H,
) -> io::Result<()>
where
    S: FnMut() -> PromptStatus,
    C: FnMut(&str) -> TurnKind,
    H: Fn(&str, &TurnCtx) -> Result<CommandOutcome, String>,
{
    let mut stdout = io::stdout();
    loop {
        let mut input = String::new();
        if io::stdin().read_line(&mut input)? == 0 {
            break;
        }
        let prompt = input.trim_end_matches(['\r', '\n']);
        if prompt.trim().is_empty() {
            continue;
        }
        if matches!(prompt.trim(), "/exit" | "/quit") {
            break;
        }
        let ctx = TurnCtx {
            cancel: Arc::new(AtomicBool::new(false)),
            interactive: false,
            from_view: false,
            progress: &|_| {},
            stream: &|_| {},
            ask_user: None,
            approve: &|_, _| false,
        };
        let text = match handler(prompt, &ctx) {
            Ok(outcome) => outcome.into_text(),
            Err(error) => format!("BŁĄD\t{error}"),
        };
        stdout.write_all(sanitize_terminal_text(&text).as_bytes())?;
        stdout.write_all(b"\n")?;
    }
    stdout.flush()
}

fn terminal_dimensions() -> (usize, usize) {
    terminal::size()
        .map(|(columns, rows)| (usize::from(columns).max(1), usize::from(rows).max(1)))
        .unwrap_or((100, 30))
}

fn editor_live_lines(
    status: &PromptStatus,
    editor: &EditorState,
    attachments: &AttachmentTray,
    slash_selection: usize,
    picker: Option<&PickerState>,
    confirm: Option<&ConfirmState>,
    columns: usize,
    rows: usize,
    color: bool,
) -> Vec<String> {
    let _capabilities = crate::capability::for_cwd(std::path::Path::new(&status.cwd));
    let width = columns.min(112).max(1);
    let mut lines = if let Some(confirm) = confirm {
        confirm_panel(confirm, width, color)
    } else if let Some(picker) = picker {
        picker_panel(picker, width, rows, color)
    } else {
        slash_hint_panel(editor.text(), width, color, slash_selection)
    };
    if picker.is_none() && confirm.is_none() {
        lines.extend(attachment_lines(attachments, width, color));
    }
    let prompt_start = lines.len();
    lines.extend(compact_prompt(width, status, editor.text(), false, color));
    lines = lines
        .into_iter()
        .flat_map(|line| line.split('\n').map(str::to_string).collect::<Vec<_>>())
        .collect();
    if picker.is_none() && confirm.is_none() {
        place_editor_cursor(
            &mut lines[prompt_start..],
            editor.text(),
            editor.cursor(),
            width,
        );
    }
    lines
}

fn park_at_live_end() -> io::Result<()> {
    let mut stdout = io::stdout();
    stdout.write_all(b"\x1b[999B\r")?;
    stdout.flush()
}

struct BracketedPasteGuard;

impl BracketedPasteGuard {
    fn enter() -> io::Result<Self> {
        crossterm::execute!(io::stdout(), EnableBracketedPaste)?;
        Ok(Self)
    }
}

impl Drop for BracketedPasteGuard {
    fn drop(&mut self) {
        let _ = crossterm::execute!(io::stdout(), DisableBracketedPaste);
    }
}

pub fn run_basic_loop<S, C, H>(
    mut status_provider: S,
    mut classify: C,
    handler: H,
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
    let mut picker: Option<PickerState> = None;
    let mut confirm: Option<ConfirmState> = None;
    let mut submission_from_view = false;
    let mut attachments = AttachmentTray::default();
    let mut follow_ups = FollowUpQueue::default();
    let runtime = RegistryUiRuntime;

    {
        let status = status_provider();
        let (columns, rows) = terminal_dimensions();
        let color = stdout_supports_color();
        let welcome = welcome_panel(
            columns.min(112),
            &status.model,
            &status.cwd,
            &status.write_status,
            &status.command_status,
            color,
        );
        let live = editor_live_lines(
            &status,
            &editor,
            &attachments,
            0,
            None,
            None,
            columns,
            rows,
            color,
        );
        renderer.flush(&welcome, &live)?;
    }

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
            let live = editor_live_lines(
                &status,
                &editor,
                &attachments,
                slash_selection,
                picker.as_ref(),
                confirm.as_ref(),
                columns,
                rows,
                color,
            );
            park_at_live_end()?;
            renderer.flush(&new_blocks, &live)?;
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
                            confirm = Some(ConfirmState::new(label, detail, command));
                            continue;
                        }
                    }
                }

                let mut submit = false;
                match key.code {
                    _ if editor.action_for(key) == Some(EditorAction::ExternalEditor)
                        && external_editor_health(Path::new(&status_provider().cwd)).is_ok() =>
                    {
                        let cwd = status_provider().cwd;
                        park_at_live_end()?;
                        renderer.flush(&[], &[])?;
                        crossterm::execute!(io::stdout(), DisableBracketedPaste)?;
                        disable_raw_mode()?;
                        let operation = OperationContext::new(
                            CancellationToken::new(),
                            ArtifactSink::new(
                                std::env::temp_dir().join("jeden-external-editor-artifacts"),
                            ),
                        );
                        let result = external_editor(&mut editor, Path::new(&cwd), &operation);
                        enable_raw_mode()?;
                        crossterm::execute!(io::stdout(), EnableBracketedPaste)?;
                        renderer.reset();
                        if let Err(error) = result {
                            messages.push(Message::new("error", error));
                        }
                        slash_selection = 0;
                    }
                    KeyCode::Esc => {
                        editor.clear();
                        slash_selection = 0;
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
                        slash_selection = 0;
                    }
                    KeyCode::Enter | KeyCode::Char('\r') | KeyCode::Char('\n') => submit = true,
                    KeyCode::Char('m') | KeyCode::Char('j')
                        if key.modifiers.contains(KeyModifiers::CONTROL) =>
                    {
                        submit = true
                    }
                    KeyCode::Tab => {
                        if let Some(completed) =
                            complete_slash_input(editor.text(), slash_selection)
                        {
                            editor.set_text(completed);
                        } else {
                            editor.insert("  ");
                        }
                        slash_selection = 0;
                    }
                    KeyCode::Right
                        if editor.cursor() == editor.text().len()
                            && !slash_matches(editor.text()).is_empty() =>
                    {
                        if let Some(completed) =
                            complete_slash_input(editor.text(), slash_selection)
                        {
                            editor.set_text(completed);
                            slash_selection = 0;
                        }
                    }
                    KeyCode::Up => {
                        let count = slash_matches(editor.text()).len();
                        if count > 0 {
                            slash_selection = if slash_selection == 0 {
                                count - 1
                            } else {
                                slash_selection - 1
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
                            slash_selection = (slash_selection + 1) % count;
                        } else if editor.text()[editor.cursor()..].contains('\n') {
                            editor.apply(EditorAction::MoveDown);
                        } else {
                            editor.apply(EditorAction::HistoryNext);
                        }
                    }
                    _ => {
                        if editor.handle_key(key) {
                            slash_selection = slash_selection
                                .min(slash_matches(editor.text()).len().saturating_sub(1));
                        }
                    }
                }

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
                let mut active_prompt = editor.take();
                attachments.clear();
                let mut active_from_view = submission_from_view;
                submission_from_view = false;
                slash_selection = 0;
                if matches!(active_prompt.trim(), "/exit" | "/quit") {
                    break;
                }
                editor.push_history(active_prompt.clone());
                messages.push(Message::new("user", active_prompt.clone()));

                {
                    let (columns, _) = terminal_dimensions();
                    let color = stdout_supports_color();
                    let mut blocks = Vec::new();
                    for message in &messages[committed..] {
                        blocks.extend(message_block(message, columns.min(112), color));
                    }
                    committed = messages.len();
                    park_at_live_end()?;
                    renderer.flush(&blocks, &[])?;
                }

                loop {
                    match classify(&active_prompt) {
                        TurnKind::Foreground => {
                            crossterm::execute!(io::stdout(), DisableBracketedPaste)?;
                            disable_raw_mode()?;
                            let ctx = TurnCtx {
                                cancel: Arc::new(AtomicBool::new(false)),
                                interactive: true,
                                from_view: active_from_view,
                                progress: &|_| {},
                                stream: &|_| {},
                                ask_user: None,
                                approve: &|_, _| false,
                            };
                            let result = handler(&active_prompt, &ctx);
                            enable_raw_mode()?;
                            crossterm::execute!(io::stdout(), EnableBracketedPaste)?;
                            renderer.reset();
                            apply_turn_result(&mut messages, &active_prompt, result, &mut picker);
                        }
                        TurnKind::Background => {
                            let steering_available = runtime
                                .availability(
                                    Path::new(&status_provider().cwd),
                                    UiFeature::Steering,
                                )
                                .available;
                            let (result, tools_used) = run_background_turn(
                                &mut renderer,
                                &handler,
                                &active_prompt,
                                active_from_view,
                                &mut editor,
                                &mut follow_ups,
                                steering_available,
                            )?;
                            if !tools_used.is_empty() {
                                messages.push(Message::new(
                                    "system",
                                    format!("tools: {}", tools_used.join(", ")),
                                ));
                            }
                            apply_turn_result(&mut messages, &active_prompt, result, &mut picker);
                        }
                    }

                    let Some(queued) = follow_ups.pop_next() else {
                        break;
                    };
                    active_prompt = queued.text;
                    active_from_view = false;
                    editor.push_history(active_prompt.clone());
                    messages.push(Message::new("user", active_prompt.clone()));
                    let (columns, _) = terminal_dimensions();
                    let color = stdout_supports_color();
                    let mut blocks = Vec::new();
                    for message in &messages[committed..] {
                        blocks.extend(message_block(message, columns.min(112), color));
                    }
                    committed = messages.len();
                    park_at_live_end()?;
                    renderer.flush(&blocks, &[])?;
                }
                needs_render = true;
            }
            _ => continue,
        }
    }

    park_at_live_end()?;
    renderer.flush(&[], &[])?;
    let mut stdout = io::stdout();
    stdout.write_all(b"\r\n")?;
    stdout.flush()
}
