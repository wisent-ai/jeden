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
    stdout_supports_color, AttachmentSource, AttachmentTray, ClipboardContent, CommandOutcome,
    ConfirmEvent, ConfirmState, EditorAction, EditorState, FollowUpQueue, Message, PickerEvent,
    PickerSpec, PickerState, PromptStatus, RegistryUiRuntime, TurnCtx, TurnKind, UiFeature,
    UiRuntimeAdapter,
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
            attachments: &[],
            progress: &|_| {},
            stream: &|_| {},
            trace: &|_| {},
            ask_user: None,
            approve: &|_, _| false,
        };
        let (text, exit) = match handler(prompt, &ctx) {
            Ok(CommandOutcome::Exit(text)) => (text, true),
            Ok(outcome) => (outcome.into_text(), false),
            Err(error) => (format!("BŁĄD\t{error}"), false),
        };
        stdout.write_all(sanitize_terminal_text(&text).as_bytes())?;
        stdout.write_all(b"\n")?;
        if exit {
            break;
        }
    }
    stdout.flush()
}

fn terminal_dimensions() -> (usize, usize) {
    terminal::size()
        .map(|(columns, rows)| (usize::from(columns).max(1), usize::from(rows).max(1)))
        .unwrap_or((100, 30))
}
fn attachment_command(
    input: &str,
    cwd: &Path,
    tray: &mut AttachmentTray,
) -> Option<Result<String, String>> {
    let trimmed = input.trim();
    let (command, rest) = trimmed
        .split_once(char::is_whitespace)
        .unwrap_or((trimmed, ""));
    let rest = rest.trim();
    match command {
        "/attach" => Some(
            tray.add_file(cwd, rest)
                .map_err(|error| error.to_string())
                .map(|id| {
                    let item = tray
                        .items()
                        .iter()
                        .find(|item| item.id == id)
                        .expect("new attachment remains in tray");
                    format!("Attached #{} {}", id.0, item.fallback_label())
                }),
        ),
        "/attachments" => Some(if rest.is_empty() {
            if tray.items().is_empty() {
                Ok("No pending attachments.".into())
            } else {
                Ok(tray
                    .items()
                    .iter()
                    .map(|item| {
                        let provenance = match &item.source {
                            AttachmentSource::Clipboard => "clipboard".to_string(),
                            AttachmentSource::File { basename } => format!("file:{basename}"),
                        };
                        format!("#{} {} · {provenance}", item.id.0, item.fallback_label())
                    })
                    .collect::<Vec<_>>()
                    .join("\n"))
            }
        } else {
            Err("Usage: /attachments".into())
        }),
        "/detach" => Some(if rest == "all" {
            let count = tray.take_all().len();
            Ok(format!("Detached {count} attachment(s)."))
        } else {
            let id = if rest.is_empty() {
                tray.items()
                    .last()
                    .map(|item| item.id)
                    .ok_or_else(|| "No pending attachments to detach.".to_string())
            } else {
                rest.strip_prefix('#')
                    .unwrap_or(rest)
                    .parse::<u64>()
                    .map(super::super::AttachmentId)
                    .map_err(|_| "Usage: /detach [id|all]".to_string())
            };
            id.and_then(|id| {
                tray.remove(id)
                    .map(|item| format!("Detached #{} {}", id.0, item.fallback_label()))
                    .ok_or_else(|| format!("Attachment #{} is not in the tray.", id.0))
            })
        }),
        _ => None,
    }
}

/// The bottom live region: an active interactive view above the prompt, or a
/// fixed prompt followed by a shrinking slash-suggestion panel.
// Each parameter is a separate borrow of independent REPL state, so there is no
// owner struct to group them behind without cloning at both call sites.
#[allow(clippy::too_many_arguments)]
fn editor_live_lines(
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
        let (live, cursor_rows_below) = editor_live_lines(
            &status,
            &editor,
            &attachments,
            0,
            picker.as_ref(),
            None,
            None,
            columns,
            rows,
            color,
        );
        renderer.flush_with_cursor(&welcome, &live, cursor_rows_below)?;
    }

    'repl: loop {
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
                let mut active_prompt = submitted;
                let mut active_attachments = attachments.take_all();
                let mut active_from_view = submission_from_view;
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

                loop {
                    match classify(&active_prompt) {
                        TurnKind::Foreground => {
                            crossterm::execute!(io::stdout(), DisableBracketedPaste)?;
                            disable_raw_mode()?;
                            let ctx = TurnCtx {
                                cancel: Arc::new(AtomicBool::new(false)),
                                interactive: true,
                                from_view: active_from_view,
                                attachments: &active_attachments,
                                progress: &|_| {},
                                stream: &|_| {},
                                trace: &|_| {},
                                ask_user: None,
                                approve: &|_, _| false,
                            };
                            let result = handler(&active_prompt, &ctx);
                            enable_raw_mode()?;
                            crossterm::execute!(io::stdout(), EnableBracketedPaste)?;
                            renderer.reset();
                            if apply_turn_result(
                                &mut messages,
                                &active_prompt,
                                result,
                                &mut picker,
                                &mut view,
                            ) {
                                break 'repl;
                            }
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
                                &active_attachments,
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
                            if apply_turn_result(
                                &mut messages,
                                &active_prompt,
                                result,
                                &mut picker,
                                &mut view,
                            ) {
                                break 'repl;
                            }
                        }
                    }

                    let Some(queued) = follow_ups.pop_next() else {
                        break;
                    };
                    active_prompt = queued.text;
                    active_attachments = Vec::new();
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
                    renderer.flush(&blocks, &[])?;
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
