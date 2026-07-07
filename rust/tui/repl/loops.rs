use std::io::{self, IsTerminal, Write};
use std::sync::atomic::AtomicBool;
use std::sync::Arc;
use std::time::Duration;

use crossterm::event::{self, Event, KeyCode, KeyEventKind, KeyModifiers};
use crossterm::terminal::{disable_raw_mode, enable_raw_mode};

use super::{live_lines, message_block, push_turn_result, RawModeGuard, ReplRenderer};
use super::background::run_background_turn;
use super::super::render::{complete_slash_input, slash_matches, welcome_panel};
use super::super::{default_columns, default_rows, render_to_stdout, stdout_supports_color, FrameOptions, Message, PromptStatus, TurnCtx, TurnKind};

fn old_read_line_loop<S, C, H>(mut status_provider: S, mut classify: C, handler: H) -> io::Result<()>
where
    S: FnMut() -> PromptStatus,
    C: FnMut(&str) -> TurnKind,
    H: Fn(&str, &TurnCtx) -> Result<String, String>,
{
    let mut messages = Vec::new();
    loop {
        let options = FrameOptions {
            status: status_provider(),
            messages: messages.clone(),
            input_text: String::new(),
            busy: false,
            columns: default_columns(),
            rows: default_rows(),
            color: stdout_supports_color(),
            slash_selection: 0,
        };
        render_to_stdout(&options)?;

        let mut input = String::new();
        let read = io::stdin().read_line(&mut input)?;
        if read == 0 {
            break;
        }

        let prompt = input.trim();
        if prompt.is_empty() {
            continue;
        }
        if matches!(prompt, "/exit" | "/quit") {
            break;
        }
        messages.push(Message::new("user", prompt));
        // Piped/non-tty: no threads, no cancel; run inline interactively.
        let _ = classify(prompt);
        let ctx = TurnCtx { cancel: Arc::new(AtomicBool::new(false)), interactive: true, progress: &|_| {}, stream: &|_| {}, approve: &|_, _| false };
        push_turn_result(&mut messages, prompt, handler(prompt, &ctx));
    }
    Ok(())
}

pub fn run_basic_loop<S, C, H>(mut status_provider: S, mut classify: C, handler: H) -> io::Result<()>
where
    S: FnMut() -> PromptStatus,
    C: FnMut(&str) -> TurnKind,
    H: Fn(&str, &TurnCtx) -> Result<String, String> + Sync,
{
    if !io::stdin().is_terminal() {
        return old_read_line_loop(status_provider, classify, handler);
    }

    let _raw = RawModeGuard::enter()?;
    let mut messages: Vec<Message> = Vec::new();
    let mut committed = 0usize; // messages already printed to scrollback
    let mut input = String::new();
    let mut slash_selection = 0usize;
    let mut needs_render = true;
    let mut renderer = ReplRenderer::new();
    // Submitted prompts, newest last; `history_index` is the cursor while
    // browsing with Up/Down (None = editing a fresh line).
    let mut history: Vec<String> = Vec::new();
    let mut history_index: Option<usize> = None;
    // Print the welcome panel once into scrollback.
    {
        let status = status_provider();
        let columns = default_columns();
        let color = stdout_supports_color();
        let welcome = welcome_panel(columns.min(120).max(50), &status.model, &status.cwd, &status.write_status, &status.command_status, color);
        renderer.flush(&welcome, &live_lines(&status, "", 0, columns, color))?;
    }
    loop {
        if needs_render {
            let status = status_provider();
            let columns = default_columns();
            let color = stdout_supports_color();
            // Commit newly-finalized messages to scrollback, then repaint only
            // the live region (slash hints + prompt).
            let mut new_blocks: Vec<String> = Vec::new();
            for message in &messages[committed..] {
                new_blocks.extend(message_block(message, columns, color));
            }
            committed = messages.len();
            let live = live_lines(&status, &input, slash_selection, columns, color);
            renderer.flush(&new_blocks, &live)?;
            needs_render = false;
        }

        if !event::poll(Duration::from_millis(250))? {
            continue;
        }
        let Event::Key(key) = event::read()? else {
            continue;
        };
        if !matches!(key.kind, KeyEventKind::Press | KeyEventKind::Repeat) {
            continue;
        }
        needs_render = true;
        match key.code {
            KeyCode::Char('c') if key.modifiers.contains(KeyModifiers::CONTROL) => break,
            KeyCode::Char('d') if key.modifiers.contains(KeyModifiers::CONTROL) && input.is_empty() => break,
            KeyCode::Esc => {
                input.clear();
                slash_selection = 0;
            }
            KeyCode::Backspace => {
                input.pop();
                slash_selection = slash_selection.min(slash_matches(&input).len().saturating_sub(1));
            }
            KeyCode::Enter if key.modifiers.contains(KeyModifiers::ALT) => {
                // Alt+Enter inserts a newline for multiline input; Enter submits.
                input.push('\n');
                slash_selection = 0;
                history_index = None;
            }
            KeyCode::Enter | KeyCode::Char('\r') | KeyCode::Char('\n') | KeyCode::Char('m') | KeyCode::Char('j')
                if matches!(key.code, KeyCode::Enter | KeyCode::Char('\r') | KeyCode::Char('\n'))
                    || key.modifiers.contains(KeyModifiers::CONTROL) => {
                if input.trim().is_empty() {
                    continue;
                }
                let prompt = input.trim().to_string();
                input.clear();
                slash_selection = 0;
                history_index = None;
                if matches!(prompt.as_str(), "/exit" | "/quit") {
                    break;
                }
                if history.last().map(|h| h != &prompt).unwrap_or(true) {
                    history.push(prompt.clone());
                }
                messages.push(Message::new("user", prompt.clone()));
                // Commit the user message (and any pending) to scrollback now, so
                // it sits above the picker/spinner that follows.
                {
                    let columns = default_columns();
                    let color = stdout_supports_color();
                    let mut blocks: Vec<String> = Vec::new();
                    for message in &messages[committed..] {
                        blocks.extend(message_block(message, columns, color));
                    }
                    committed = messages.len();
                    renderer.flush(&blocks, &[])?;
                }
                match classify(&prompt) {
                    TurnKind::Foreground => {
                        disable_raw_mode()?;
                        let ctx = TurnCtx { cancel: Arc::new(AtomicBool::new(false)), interactive: true, progress: &|_| {}, stream: &|_| {}, approve: &|_, _| false };
                        let result = handler(&prompt, &ctx);
                        enable_raw_mode()?;
                        renderer.reset();
                        push_turn_result(&mut messages, &prompt, result);
                    }
                    TurnKind::Background => {
                        let (result, tools_used) = run_background_turn(&mut renderer, &handler, &prompt)?;
                        if !tools_used.is_empty() {
                            messages.push(Message::new("system", format!("tools: {}", tools_used.join(", "))));
                        }
                        push_turn_result(&mut messages, &prompt, result);
                    }
                }
            }
            KeyCode::Char(ch) => {
                input.push(ch);
                slash_selection = 0;
                history_index = None;
            }
            KeyCode::Up => {
                let count = slash_matches(&input).len();
                if count > 0 {
                    slash_selection = if slash_selection == 0 { count - 1 } else { slash_selection - 1 };
                } else if !history.is_empty() {
                    let idx = match history_index {
                        None => history.len() - 1,
                        Some(0) => 0,
                        Some(i) => i - 1,
                    };
                    history_index = Some(idx);
                    input = history[idx].clone();
                }
            }
            KeyCode::Down => {
                let count = slash_matches(&input).len();
                if count > 0 {
                    slash_selection = (slash_selection + 1) % count;
                } else if let Some(i) = history_index {
                    if i + 1 < history.len() {
                        history_index = Some(i + 1);
                        input = history[i + 1].clone();
                    } else {
                        history_index = None;
                        input.clear();
                    }
                }
            }
            KeyCode::Tab | KeyCode::Right => {
                if let Some(completed) = complete_slash_input(&input, slash_selection) {
                    input = completed;
                    slash_selection = 0;
                }
            }
            _ => {}
        }
    }

    let mut stdout = io::stdout();
    stdout.write_all(b"\x1b[?25h\n")?;
    stdout.flush()
}
