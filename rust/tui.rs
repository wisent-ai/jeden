use std::io::{self, IsTerminal, Write};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc;
use std::sync::Arc;
use std::thread;
use std::time::Duration;

use crossterm::event::{self, Event, KeyCode, KeyEventKind, KeyModifiers};
use crossterm::terminal::{disable_raw_mode, enable_raw_mode};

const PRODUCT: &str = "Wisent";
const APP: &str = "Agent";
const VERSION: &str = "v0.1.0";
const ASSISTANT_TITLE: &str = "wisent";

const WISENT_MARK: &[&str] = &[
    "        ▄▄▄██▀▀▀▀▀▀██▄▄▄",
    "     ▄█▀▀             ▀▀▀█▄",
    "  ▄██▀                    ▀██",
    " ▄█▀▀▀█▄▄                   ▀█▄",
    "▄█▀     ▀▀▀█▄▄▄              ▀█▄",
    "██▄▄          ▀▀▀██▄▄▄        ██",
    "██▀▀██               ▀▀▀▀██▄▄▄██",
    "▀█▄ ██                       ▄█▀",
    " ▀█▄ ██              ▄▄▄    ▄█▀",
    "   ▀█▄██▄          ▄█▀▀▀▀▀███▀",
    "     ▀████▄    ▄▄██▀  ▄▄▄█▀",
    "        ▀▀▀██████▄▄██▀▀▀",
];



const SLASH_COMMAND_HINTS: &[(&str, &str)] = &[
    ("login", "Automated OAuth login"), ("logout", "Logout provider"), ("model", "Switch model"),
    ("help", "Show slash commands"), ("mcp", "Manage MCP servers"), ("settings", "Open settings menu"), ("setup", "Open provider setup"),
    ("plan", "Toggle plan mode"), ("plan-review", "Review latest plan"), ("goal", "Toggle goal mode"),
    ("loop", "Toggle loop mode"), ("fast", "Toggle priority service tier"), ("advisor", "Toggle advisor reviewer"),
    ("export", "Export session"), ("dump", "Dump session"), ("share", "Share session"),
    ("collab", "Collaborate via relay"), ("join", "Join shared session"), ("leave", "Leave collab"),
    ("browser", "Configure browser runtime"), ("copy", "Copy conversation text"), ("todo", "Manage todos"),
    ("session", "Session management"), ("jobs", "Show jobs"), ("usage", "Show provider usage"),
    ("stats", "Launch stats dashboard"), ("changelog", "Show changelog"), ("hotkeys", "Show hotkeys"),
    ("tools", "Show tools"), ("context", "Show context usage"), ("extensions", "Manage extensions"),
    ("agents", "Agent controls"), ("branch", "Create branch"), ("fork", "Create fork"), ("tree", "Navigate tree"),
    ("ssh", "Manage SSH hosts"), ("new", "Start new session"), ("fresh", "Reset provider stream state"),
    ("drop", "Drop current session"), ("compact", "Compact session"), ("shake", "Shake session context"),
    ("handoff", "Hand off session"), ("resume", "Resume session"), ("btw", "Side question"),
    ("tan", "Background agent"), ("omfg", "Forge local rule"), ("retry", "Retry last failed turn"),
    ("debug", "Open debug tools"), ("memory", "Memory maintenance"), ("rename", "Rename session"),
    ("move", "Move session workspace"), ("marketplace", "Manage marketplace plugins"),
    ("plugins", "Manage installed plugins"), ("reload-plugins", "Reload plugins"),
    ("update", "Run automated update"), ("force", "Force next tool"), ("exit", "Exit"), ("quit", "Quit"),
    ("commands", "Show slash commands"),
];

#[derive(Debug, Clone)]
pub struct Message {
    pub role: String,
    pub text: String,
}

impl Message {
    pub fn new(role: impl Into<String>, text: impl Into<String>) -> Self {
        Self { role: role.into(), text: text.into() }
    }
}

#[derive(Debug, Clone)]
pub struct PromptStatus {
    pub cwd: String,
    pub write_status: String,
    pub command_status: String,
    pub model: String,
    pub service_tier: String,
    pub branch: Option<String>,
    pub dirty_count: usize,
    pub context_percent: Option<f64>,
    pub context_limit: Option<String>,
    pub cost: Option<String>,
}


#[derive(Debug, Clone)]
pub struct FrameOptions {
    pub status: PromptStatus,
    pub messages: Vec<Message>,
    pub input_text: String,
    pub busy: bool,
    pub columns: usize,
    pub rows: usize,
    pub color: bool,
    pub slash_selection: usize,
}

pub fn default_columns() -> usize {
    std::env::var("COLUMNS").ok().and_then(|value| value.parse().ok()).unwrap_or(100)
}

pub fn default_rows() -> usize {
    std::env::var("LINES").or_else(|_| std::env::var("ROWS")).ok().and_then(|value| value.parse().ok()).unwrap_or(30)
}

pub fn stdout_supports_color() -> bool {
    io::stdout().is_terminal() && std::env::var_os("NO_COLOR").is_none()
}

fn paint(value: &str, color: &str, enabled: bool) -> String {
    if !enabled {
        return value.to_string();
    }
    let code = match color {
        "dim" => "\x1b[2m",
        "bold" => "\x1b[1m",
        "cyan" => "\x1b[36m",
        "green" => "\x1b[32m",
        "yellow" => "\x1b[33m",
        "magenta" => "\x1b[35m",
        "red" => "\x1b[31m",
        _ => "",
    };
    format!("{}{}\x1b[0m", code, value)
}

fn visible_len(value: &str) -> usize {
    let mut len = 0;
    let mut chars = value.chars().peekable();
    while let Some(ch) = chars.next() {
        if ch == '\x1b' && chars.peek() == Some(&'[') {
            chars.next();
            for code in chars.by_ref() {
                if code == 'm' {
                    break;
                }
            }
        } else {
            len += 1;
        }
    }
    len
}

fn take_visible(value: &str, max: usize) -> String {
    value.chars().take(max).collect()
}

fn pad_visible(value: &str, width: usize) -> String {
    let extra = width.saturating_sub(visible_len(value));
    format!("{}{}", value, " ".repeat(extra))
}

fn wrap_line(line: &str, width: usize) -> Vec<String> {
    if visible_len(line) <= width {
        return vec![line.to_string()];
    }
    let chars: Vec<char> = line.chars().collect();
    if width == 0 {
        return vec![String::new()];
    }
    chars.chunks(width).map(|chunk| chunk.iter().collect()).collect()
}

fn compact_path(cwd: &str) -> String {
    let parts = cwd.split('/').filter(|part| !part.is_empty()).collect::<Vec<_>>();
    if parts.len() >= 2 {
        format!("…/{}/{}", parts[parts.len() - 2], parts[parts.len() - 1])
    } else {
        cwd.to_string()
    }
}

fn clamp_visible(value: &str, width: usize) -> String {
    if visible_len(value) > width {
        format!("{}…", take_visible(value, width.saturating_sub(1)))
    } else {
        value.to_string()
    }
}


fn boxed(title: &str, rows: &[String], width: usize, color: bool) -> Vec<String> {
    let clean_title = format!(" {} ", title);
    let inner = width.saturating_sub(4).max(clean_title.chars().count() + 2).max(8);
    let mut normalized = Vec::new();
    for row in rows {
        for part in row.split('\n') {
            normalized.extend(wrap_line(part, inner));
        }
    }
    let top = format!(
        "{}{}{}{}",
        paint("╭", "cyan", color),
        paint(&clean_title, "bold", color),
        paint(&"─".repeat((inner + 2).saturating_sub(clean_title.chars().count())), "cyan", color),
        paint("╮", "cyan", color)
    );
    let mut out = vec![top];
    for row in normalized {
        out.push(format!("{} {} {}", paint("│", "cyan", color), pad_visible(&row, inner), paint("│", "cyan", color)));
    }
    out.push(format!("{}{}{}", paint("╰", "cyan", color), paint(&"─".repeat(inner + 2), "cyan", color), paint("╯", "cyan", color)));
    out
}

fn role_color(role: &str) -> &'static str {
    match role {
        "assistant" => "magenta",
        "error" => "red",
        "system" => "yellow",
        _ => "cyan",
    }
}

fn format_message(message: &Message, width: usize, color: bool) -> Vec<String> {
    let title = if message.role == "assistant" { ASSISTANT_TITLE } else { message.role.as_str() };
    boxed(title, &message.text.split('\n').map(str::to_string).collect::<Vec<_>>(), width, color)
        .into_iter()
        .map(|line| paint(&line, role_color(&message.role), color))
        .collect()
}

fn welcome_panel(width: usize, model: &str, cwd: &str, write_status: &str, command_status: &str, color: bool) -> Vec<String> {
    let title = format!("{} {} {}", PRODUCT, APP, VERSION);
    let inner = width.saturating_sub(4).max(48);
    let left_width = (inner / 3).clamp(24, 34);
    let right_width = inner.saturating_sub(left_width + 3).max(24);
    let cwd_label = compact_path(cwd);
    let mut left = vec![
        String::new(),
        "Welcome back!".to_string(),
        String::new(),
    ];
    left.extend(WISENT_MARK.iter().map(|line| line.to_string()));
    left.extend([
        String::new(),
        if model.is_empty() { "default".to_string() } else { model.to_string() },
        "Jeden CLI".to_string(),
    ]);
    let right = [
        "Tips".to_string(),
        "Type a task and press Enter".to_string(),
        "/help for commands".to_string(),
        "/model to switch routes".to_string(),
        "/update runs automated self-update".to_string(),
        "! and $ shells are not wired yet".to_string(),
        "────────────────────────".to_string(),
        format!("Workspace: {}", cwd_label),
        format!("Tool gates: write {} · command {}", write_status, command_status),
        "CLI: jeden sessions".to_string(),
        "CLI: jeden artifacts <id>".to_string(),
    ];
    let mut rows = Vec::new();
    for index in 0..left.len().max(right.len()) {
        let left_cell = clamp_visible(&left.get(index).cloned().unwrap_or_default(), left_width);
        let right_cell = clamp_visible(&right.get(index).cloned().unwrap_or_default(), right_width);
        rows.push(format!(
            "{} │ {}",
            pad_visible(&left_cell, left_width),
            pad_visible(&right_cell, right_width)
        ));
    }
    boxed(&title, &rows, width, color)
}


fn slash_query(input_text: &str) -> Option<String> {
    let text = input_text.trim_start();
    if !text.starts_with('/') || text.contains('\n') {
        return None;
    }
    let query = text.trim_start_matches('/');
    if query.contains(char::is_whitespace) {
        return None;
    }
    Some(query.to_ascii_lowercase())
}

fn slash_matches(input_text: &str) -> Vec<(&'static str, &'static str)> {
    let Some(prefix) = slash_query(input_text) else {
        return Vec::new();
    };
    SLASH_COMMAND_HINTS
        .iter()
        .filter(|(name, _)| name.starts_with(&prefix))
        .take(6)
        .copied()
        .collect()
}

fn complete_slash_input(input_text: &str, selected: usize) -> Option<String> {
    let matches = slash_matches(input_text);
    let (name, _) = matches.get(selected.min(matches.len().saturating_sub(1)))?;
    Some(format!("/{name} "))
}

fn slash_hint_panel(input_text: &str, width: usize, color: bool, selected: usize) -> Vec<String> {
    let matches = slash_matches(input_text);
    if matches.is_empty() {
        return Vec::new();
    }
    let selected = selected.min(matches.len().saturating_sub(1));
    let rows: Vec<String> = matches
        .iter()
        .enumerate()
        .map(|(index, (name, description))| {
            let marker = if index == selected { "›" } else { " " };
            format!("{marker} /{:<15} — {}", name, description)
        })
        .collect();
    boxed("slash suggestions", &rows, width, color)
}

fn compact_prompt(width: usize, status: &PromptStatus, input_text: &str, _busy: bool, color: bool) -> Vec<String> {
    let inner = width.saturating_sub(2).max(48);
    let model = if status.model.is_empty() { "default" } else { status.model.as_str() };
    let tier = if status.service_tier.is_empty() { "default" } else { status.service_tier.as_str() };
    let branch = status
        .branch
        .as_ref()
        .map(|branch| format!(" > ⑂ {}{}", branch, if status.dirty_count > 0 { format!(" ?{}", status.dirty_count) } else { String::new() }))
        .unwrap_or_default();
    let context = match (status.context_percent, status.context_limit.as_deref()) {
        (Some(percent), Some(limit)) => format!(" > ◫ {:.1}%/{}", percent, limit),
        (_, Some(limit)) => format!(" > ◫ {}", limit),
        _ => String::new(),
    };
    let cost = status.cost.as_ref().map(|cost| format!(" > {}", cost)).unwrap_or_default();
    let label = format!(
        " jeden > ⬢ {} > ↯ {} > 📁 {}{}{}{} ▶ ",
        model,
        tier,
        compact_path(&status.cwd),
        branch,
        context,
        cost
    );
    let safe_label = if visible_len(&label) > inner.saturating_sub(4) {
        format!("{}… ▶ ", take_visible(&label, inner.saturating_sub(7)))
    } else {
        label
    };
    let top = format!(
        "{}{}{}{}",
        paint("╭──", "cyan", color),
        safe_label,
        paint(&"─".repeat(inner.saturating_sub(visible_len(&safe_label) + 2)), "cyan", color),
        paint("╮", "cyan", color)
    );
    // Render the (possibly multiline) input: first line after the ╰─ caret,
    // continuation lines indented under it. Single-line input is unchanged.
    let mut out = vec![top];
    let input_lines: Vec<&str> = input_text.split('\n').collect();
    for (index, line) in input_lines.iter().enumerate() {
        if index == 0 {
            out.push(format!("{} {}", paint("╰─", "cyan", color), line));
        } else {
            out.push(format!("   {}", line));
        }
    }
    out
}

fn frame_lines(options: &FrameOptions) -> Vec<String> {
    let width = options.columns.min(120).max(50);
    let prompt = compact_prompt(width, &options.status, &options.input_text, options.busy, options.color);
    let slash_hints = slash_hint_panel(&options.input_text, width, options.color, options.slash_selection);
    let reserved = prompt.len() + slash_hints.len() + 1;
    let available_rows = options.rows.saturating_sub(reserved).max(4);
    let message_lines: Vec<String> = options.messages.iter().flat_map(|message| format_message(message, width, options.color)).collect();
    let mut main_lines = if message_lines.is_empty() {
        welcome_panel(width, &options.status.model, &options.status.cwd, &options.status.write_status, &options.status.command_status, options.color)
    } else {
        message_lines
    };
    if main_lines.len() > available_rows {
        main_lines = main_lines.split_off(main_lines.len() - available_rows);
    }
    let mut lines = Vec::new();
    lines.extend(main_lines);
    lines.extend(std::iter::repeat(String::new()).take(available_rows.saturating_sub(lines.len())));
    lines.extend(slash_hints);
    lines.extend(prompt);
    // Never let an embedded newline reach the absolute-positioned diff renderer.
    lines
        .into_iter()
        .flat_map(|line| line.split('\n').map(str::to_string).collect::<Vec<_>>())
        .collect()
}

pub fn render_terminal_frame(options: &FrameOptions) -> String {
    let mut lines = vec!["\x1b[2J\x1b[H".to_string()];
    lines.extend(frame_lines(options));
    lines.join("\n")
}

pub fn render_to_stdout(options: &FrameOptions) -> io::Result<()> {
    let mut stdout = io::stdout();
    stdout.write_all(render_terminal_frame(options).as_bytes())?;
    stdout.write_all(b"\x1b[?25h")?;
    stdout.flush()
}

/// Differential, absolute-positioned renderer for raw-mode interactive use.
///
/// Raw mode disables ONLCR, so a bare `\n` moves the cursor down without a
/// carriage return, and full-width lines autowrap. Both corrupt the layout.
/// This renderer positions every line with an absolute cursor move, disables
/// autowrap for the paint, and rewrites only the rows that changed since the
/// previous frame — so idle/keystroke updates never clear or scroll the screen.
struct DiffRenderer {
    previous: Vec<String>,
    columns: usize,
    rows: usize,
}

impl DiffRenderer {
    fn new() -> Self {
        Self { previous: Vec::new(), columns: 0, rows: 0 }
    }

    /// Force a full repaint on the next `draw` (after a child process wrote to
    /// the screen, e.g. an interactive provider selector).
    fn reset(&mut self) {
        self.previous.clear();
    }

    fn draw(&mut self, options: &FrameOptions) -> io::Result<()> {
        let lines = frame_lines(options);
        let size_changed = self.columns != options.columns || self.rows != options.rows;
        let out = compose_update(&self.previous, &lines, options.columns.max(1), size_changed);

        self.previous = lines;
        self.columns = options.columns;
        self.rows = options.rows;

        let mut stdout = io::stdout();
        stdout.write_all(out.as_bytes())?;
        stdout.flush()
    }
}

/// Pure ANSI generator for one differential frame update.
///
/// A full paint (first frame, row-count change, or terminal resize) clears the
/// screen and writes every row; otherwise only rows that differ from `previous`
/// are rewritten. Every row is placed with an absolute cursor move + line clear,
/// never a bare `\n`, so raw mode (ONLCR off) can never stair-step or wrap it.
fn compose_update(previous: &[String], lines: &[String], columns: usize, size_changed: bool) -> String {
    let full = previous.len() != lines.len() || size_changed;
    let mut out = String::new();
    // Hide cursor and disable autowrap for the duration of the paint.
    out.push_str("\x1b[?25l\x1b[?7l");
    if full {
        out.push_str("\x1b[2J\x1b[H");
        for (index, line) in lines.iter().enumerate() {
            out.push_str(&format!("\x1b[{};1H\x1b[2K{}", index + 1, line));
        }
    } else {
        for (index, line) in lines.iter().enumerate() {
            if previous[index] != *line {
                out.push_str(&format!("\x1b[{};1H\x1b[2K{}", index + 1, line));
            }
        }
    }
    // Re-enable autowrap, park the cursor after the prompt text, show it.
    let last_row = lines.len().max(1);
    let last_col = (visible_len(lines.last().map(String::as_str).unwrap_or("")) + 1).min(columns.max(1));
    out.push_str(&format!("\x1b[?7h\x1b[{};{}H\x1b[?25h", last_row, last_col));
    out
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

/// How a submitted line should run relative to the terminal.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TurnKind {
    /// Runs inline on the main thread with raw mode suspended — for commands
    /// that need the cooked terminal (interactive `omp auth-broker login`) or
    /// that return instantly.
    Foreground,
    /// Runs on a worker thread while the TUI stays live (spinner + Esc-cancel).
    Background,
}

/// Default policy: agent turns (plain prompts, `/retry`, `/btw`) run in the
/// background so the TUI stays live; every other slash command runs inline
/// (some need the cooked terminal, most return instantly).
pub fn default_turn_kind(input: &str) -> TurnKind {
    let trimmed = input.trim();
    if !trimmed.starts_with('/') {
        return TurnKind::Background;
    }
    let command = trimmed.split_whitespace().next().unwrap_or(trimmed);
    match command {
        "/retry" | "/btw" => TurnKind::Background,
        _ => TurnKind::Foreground,
    }
}

/// Worker→render-loop message during a background turn.
enum TurnMsg {
    /// Spinner status line ("thinking…", "tool: read_file").
    Note(String),
    /// A chunk of live assistant text.
    Delta(String),
}

/// Cooperative controls handed to a turn handler.
pub struct TurnCtx<'a> {
    /// Set true when the user presses Esc/Ctrl-C during a background turn.
    pub cancel: Arc<AtomicBool>,
    /// False on a background turn: stdin-reading tools must refuse.
    pub interactive: bool,
    /// Live status sink rendered next to the spinner.
    pub progress: &'a dyn Fn(&str),
    /// Per-token streaming sink for live assistant text.
    pub stream: &'a dyn Fn(&str),
}

fn spinner_glyph(frame: usize) -> char {
    const FRAMES: [char; 10] = ['⠋', '⠙', '⠹', '⠸', '⠼', '⠴', '⠦', '⠧', '⠇', '⠏'];
    FRAMES[frame % FRAMES.len()]
}

fn push_turn_result(messages: &mut Vec<Message>, prompt: &str, result: Result<String, String>) {
    match result {
        Ok(text) => messages.push(Message::new(if prompt.starts_with('/') { "system" } else { "assistant" }, text.trim().to_string())),
        Err(error) => messages.push(Message::new("error", error)),
    }
}

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
        let ctx = TurnCtx { cancel: Arc::new(AtomicBool::new(false)), interactive: true, progress: &|_| {}, stream: &|_| {} };
        push_turn_result(&mut messages, prompt, handler(prompt, &ctx));
    }
    Ok(())
}

/// Run a background turn on a worker thread while animating a spinner and
/// draining live progress. Esc / Ctrl-C set the shared cancel flag, which the
/// agent loop polls between steps. Returns the handler's result.
fn run_background_turn<S, H>(
    renderer: &mut DiffRenderer,
    status_provider: &mut S,
    messages: &[Message],
    handler: &H,
    prompt: &str,
) -> io::Result<(Result<String, String>, Vec<String>)>
where
    S: FnMut() -> PromptStatus,
    H: Fn(&str, &TurnCtx) -> Result<String, String> + Sync,
{
    let cancel = Arc::new(AtomicBool::new(false));
    // Note = spinner status line; Delta = a live assistant text chunk.
    let (tx, rx) = mpsc::channel::<TurnMsg>();
    let mut note = String::from("working…");
    let mut streamed = String::new();
    let mut frame = 0usize;
    // Distinct tool names this turn ran, in first-seen order, for a transcript
    // line so tool execution is visible (not just a vanishing spinner).
    let mut tools_used: Vec<String> = Vec::new();
    let record_tool = |message: &str, tools: &mut Vec<String>| {
        if let Some(tool) = message.strip_prefix("tool: ") {
            let tool = tool.trim().to_string();
            if !tool.is_empty() && !tools.contains(&tool) {
                tools.push(tool);
            }
        }
    };
    // Status (git branch, model, cwd) is stable for the duration of a turn;
    // snapshot it once so the ~8fps spinner loop does not re-shell git per frame.
    let status = status_provider();

    let outcome = thread::scope(|scope| -> io::Result<Result<String, String>> {
        let worker_cancel = cancel.clone();
        let note_tx = tx.clone();
        let delta_tx = tx.clone();
        let worker = scope.spawn(move || {
            let progress = move |message: &str| {
                let _ = note_tx.send(TurnMsg::Note(message.to_string()));
            };
            let stream = move |piece: &str| {
                let _ = delta_tx.send(TurnMsg::Delta(piece.to_string()));
            };
            let ctx = TurnCtx { cancel: worker_cancel, interactive: false, progress: &progress, stream: &stream };
            handler(prompt, &ctx)
        });
        drop(tx);

        loop {
            while let Ok(message) = rx.try_recv() {
                match message {
                    TurnMsg::Note(m) => { record_tool(&m, &mut tools_used); note = m; }
                    TurnMsg::Delta(p) => { streamed.push_str(&p); }
                }
            }
            let cancelling = cancel.load(Ordering::Relaxed);
            let label = if cancelling {
                format!("{} cancelling…", spinner_glyph(frame))
            } else {
                format!("{} {} · esc to cancel", spinner_glyph(frame), note)
            };
            let mut view = messages.to_vec();
            // Show streamed assistant text live above the spinner as it arrives.
            if !streamed.trim().is_empty() {
                view.push(Message::new("assistant", streamed.clone()));
            }
            view.push(Message::new("system", label));
            let options = FrameOptions {
                status: status.clone(),
                messages: view,
                input_text: String::new(),
                busy: true,
                columns: default_columns(),
                rows: default_rows(),
                color: stdout_supports_color(),
                slash_selection: 0,
            };
            renderer.draw(&options)?;
            frame = frame.wrapping_add(1);

            if worker.is_finished() {
                break;
            }
            if event::poll(Duration::from_millis(120))? {
                if let Event::Key(key) = event::read()? {
                    if matches!(key.kind, KeyEventKind::Press | KeyEventKind::Repeat) {
                        let is_ctrl_c = key.code == KeyCode::Char('c') && key.modifiers.contains(KeyModifiers::CONTROL);
                        if key.code == KeyCode::Esc || is_ctrl_c {
                            cancel.store(true, Ordering::Relaxed);
                        }
                    }
                }
            }
        }

        // Drain any final messages and join.
        while let Ok(message) = rx.try_recv() {
            match message {
                TurnMsg::Note(m) => { record_tool(&m, &mut tools_used); note = m; }
                TurnMsg::Delta(p) => { streamed.push_str(&p); }
            }
        }
        let _ = note;
        Ok(worker.join().unwrap_or_else(|_| Err("Turn thread panicked.".into())))
    })?;

    renderer.reset();
    Ok((outcome, tools_used))
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
    let mut messages = Vec::new();
    let mut input = String::new();
    let mut slash_selection = 0usize;
    let mut needs_render = true;
    let mut renderer = DiffRenderer::new();
    // Submitted prompts, newest last; `history_index` is the cursor while
    // browsing with Up/Down (None = editing a fresh line).
    let mut history: Vec<String> = Vec::new();
    let mut history_index: Option<usize> = None;
    loop {
        if needs_render {
            let options = FrameOptions {
                status: status_provider(),
                messages: messages.clone(),
                input_text: input.clone(),
                busy: false,
                columns: default_columns(),
                rows: default_rows(),
                color: stdout_supports_color(),
                slash_selection,
            };
            renderer.draw(&options)?;
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
                match classify(&prompt) {
                    TurnKind::Foreground => {
                        disable_raw_mode()?;
                        let ctx = TurnCtx { cancel: Arc::new(AtomicBool::new(false)), interactive: true, progress: &|_| {}, stream: &|_| {} };
                        let result = handler(&prompt, &ctx);
                        enable_raw_mode()?;
                        renderer.reset();
                        push_turn_result(&mut messages, &prompt, result);
                    }
                    TurnKind::Background => {
                        let (result, tools_used) = run_background_turn(&mut renderer, &mut status_provider, &messages, &handler, &prompt)?;
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

#[cfg(test)]
mod tests {
    use super::*;

    fn test_status() -> PromptStatus {
        PromptStatus {
            cwd: "/tmp/work".to_string(),
            write_status: "ask".to_string(),
            command_status: "ask".to_string(),
            model: "test-model".to_string(),
            service_tier: "priority".to_string(),
            branch: Some("main".to_string()),
            dirty_count: 1,
            context_percent: None,
            context_limit: Some("2048".to_string()),
            cost: None,
        }
    }

    fn render_with_input(input_text: &str, slash_selection: usize) -> String {
        render_terminal_frame(&FrameOptions {
            status: test_status(),
            messages: vec![Message::new("assistant", "ready")],
            input_text: input_text.to_string(),
            busy: false,
            columns: 80,
            rows: 24,
            color: false,
            slash_selection,
        })
    }


    #[test]
    fn renders_welcome_and_prompt() {
        let frame = render_terminal_frame(&FrameOptions {
            status: PromptStatus {
                cwd: "/tmp/work".to_string(),
                write_status: "ask".to_string(),
                command_status: "ask".to_string(),
                model: "test-model".to_string(),
                service_tier: "priority".to_string(),
                branch: Some("main".to_string()),
                dirty_count: 1,
                context_percent: None,
                context_limit: Some("2048".to_string()),
                cost: None,
            },
            messages: Vec::new(),
            input_text: String::new(),
            busy: false,
            columns: 80,
            rows: 24,
            color: false,
            slash_selection: 0,
        });
        assert!(frame.contains("Wisent Agent v0.1.0"));
        assert!(frame.contains("Tool gates: write ask · command ask"));
        assert!(frame.contains("jeden > ⬢ test-model"));
        assert!(frame.contains("Welcome back!"));
        assert!(frame.contains("/update runs automated self-update"));
        assert!(frame.ends_with("╰─ "));
    }

    #[test]
    fn slash_input_renders_suggestions_and_keeps_typed_slash_in_prompt() {
        let frame = render_with_input("/", 0);

        assert!(frame.contains("slash suggestions"), "{frame}");
        assert!(frame.contains("› /login"), "{frame}");
        assert!(frame.contains("  /logout"), "{frame}");
        assert!(frame.ends_with("╰─ /"), "{frame}");
    }

    #[test]
    fn slash_prefix_filters_plan_commands_before_enter() {
        let frame = render_with_input("/pl", 0);

        assert!(frame.contains("slash suggestions"), "{frame}");
        assert!(frame.contains("› /plan"), "{frame}");
        assert!(frame.contains("  /plan-review"), "{frame}");
        assert!(!frame.contains("/settings"), "{frame}");
        assert!(frame.ends_with("╰─ /pl"), "{frame}");
    }

    #[test]
    fn slash_tab_completion_accepts_selected_suggestion() {
        assert_eq!(complete_slash_input("/pl", 0), Some("/plan ".to_string()));
        assert_eq!(complete_slash_input("/pl", 1), Some("/plan-review ".to_string()));
    }

    fn frame_options(input_text: &str, messages: Vec<Message>, columns: usize, rows: usize) -> FrameOptions {
        FrameOptions {
            status: test_status(),
            messages,
            input_text: input_text.to_string(),
            busy: false,
            columns,
            rows,
            color: false,
            slash_selection: 0,
        }
    }

    #[test]
    fn frame_lines_never_embed_a_newline_for_multiline_message() {
        // Raw mode turns a bare `\n` into a bare line-feed (no carriage return),
        // so the frame the diff renderer positions must be one row per element.
        let options = frame_options(
            "",
            vec![
                Message::new("assistant", "line-a\nline-b"),
                Message::new("user", "just one line"),
            ],
            80,
            24,
        );
        let lines = frame_lines(&options);
        assert!(
            lines.iter().all(|line| !line.contains('\n')),
            "an embedded newline reached the absolute-positioned renderer: {lines:?}"
        );
        // The two halves of the multi-line message must occupy distinct rows,
        // proving the newline became a real row break rather than being dropped.
        let first = lines.iter().position(|line| line.contains("line-a"));
        let second = lines.iter().position(|line| line.contains("line-b"));
        assert!(first.is_some() && second.is_some(), "both message halves must render: {lines:?}");
        assert_ne!(first, second, "multi-line content must split onto separate rows: {lines:?}");
    }

    #[test]
    fn frame_lines_split_a_newline_carried_by_raw_prompt_input() {
        // `compact_prompt` embeds `input_text` verbatim, so a newline typed (or
        // pasted) into the prompt reaches the frame un-split by the message path.
        // Only the frame_lines backstop keeps it off the absolute-positioned
        // renderer — this is the row the ticket's split exists to defend.
        let options = frame_options("first\nsecond", vec![Message::new("assistant", "ready")], 80, 24);
        let lines = frame_lines(&options);
        assert!(
            lines.iter().all(|line| !line.contains('\n')),
            "a newline pasted into the prompt leaked past the backstop: {lines:?}"
        );
        let first = lines.iter().position(|line| line.contains("first"));
        let second = lines.iter().position(|line| line.contains("second"));
        assert!(first.is_some() && second.is_some(), "both prompt fragments must render: {lines:?}");
        assert_ne!(first, second, "the pasted newline must break onto a separate row: {lines:?}");
    }

    #[test]
    fn frame_lines_never_exceed_the_effective_render_width() {
        // The renderer clamps columns to [50, 120]; any wider row would autowrap
        // in raw mode and stair-step the layout. A 400-char message forces the
        // wrap path across the lower clamp, mid range, and the upper clamp.
        let wide_message = "x".repeat(400);
        for columns in [30usize, 50, 80, 100, 120, 200] {
            let effective = columns.min(120).max(50);
            let options = frame_options(
                "/plan",
                vec![Message::new("assistant", wide_message.clone())],
                columns,
                40,
            );
            for line in frame_lines(&options) {
                let width = visible_len(&line);
                assert!(
                    width <= effective,
                    "columns={columns} effective={effective}: line of visible width {width} would autowrap: {line:?}"
                );
            }
        }
    }

    #[test]
    fn frame_lines_show_slash_suggestions_and_keep_typed_input_in_prompt() {
        let options = frame_options("/", vec![Message::new("assistant", "ready")], 80, 24);
        let lines = frame_lines(&options);
        assert!(
            lines.iter().any(|line| line.contains("slash suggestions")),
            "slash suggestion panel missing when input is '/': {lines:?}"
        );
        let prompt = lines.last().expect("frame_lines always ends with the prompt row");
        assert!(
            prompt.ends_with("╰─ /"),
            "prompt row must end with the typed input: {prompt:?}"
        );
    }

    #[test]
    fn compose_update_first_frame_clears_once_and_uses_no_bare_newline() {
        let lines = vec!["alpha".to_string(), "beta".to_string(), "gamma".to_string()];
        let out = compose_update(&[], &lines, 80, false);
        assert_eq!(
            out.matches("\x1b[2J").count(),
            1,
            "the first frame must clear the screen exactly once: {out:?}"
        );
        assert!(
            !out.contains('\n'),
            "raw mode forbids bare newlines in the emitted update: {out:?}"
        );
        for (index, line) in lines.iter().enumerate() {
            assert!(
                out.contains(&format!("\x1b[{};1H\x1b[2K{}", index + 1, line)),
                "row {index} was not placed with an absolute cursor move: {out:?}"
            );
        }
    }

    #[test]
    fn compose_update_incremental_rewrites_only_the_changed_row() {
        let previous = vec!["alpha".to_string(), "beta".to_string(), "gamma".to_string()];
        let mut next = previous.clone();
        next[2] = "gamma-2".to_string();
        let out = compose_update(&previous, &next, 80, false);
        assert_eq!(
            out.matches("\x1b[2J").count(),
            0,
            "an in-place update must never clear the whole screen: {out:?}"
        );
        assert_eq!(
            out.matches("\x1b[2K").count(),
            1,
            "exactly one row (the changed one) may be rewritten: {out:?}"
        );
        assert!(
            out.contains("\x1b[3;1H\x1b[2Kgamma-2"),
            "the changed row must be rewritten at its absolute position: {out:?}"
        );
        assert!(
            !out.contains("alpha") && !out.contains("beta"),
            "unchanged rows must not be repainted: {out:?}"
        );
    }

    #[test]
    fn compose_update_size_change_forces_a_full_paint() {
        // Same length, identical content: only `size_changed` should force a
        // full clear+repaint. Without it this call would emit nothing to rewrite.
        let previous = vec!["alpha".to_string(), "beta".to_string()];
        let identical = previous.clone();
        let out = compose_update(&previous, &identical, 80, true);
        assert_eq!(
            out.matches("\x1b[2J").count(),
            1,
            "a size change must clear and fully repaint: {out:?}"
        );
        for (index, line) in identical.iter().enumerate() {
            assert!(
                out.contains(&format!("\x1b[{};1H\x1b[2K{}", index + 1, line)),
                "row {index} was not fully repainted after resize: {out:?}"
            );
        }
    }

    #[test]
    fn default_turn_kind_routes_prompts_and_slash_commands() {
        // Agent turns (plain prompts, `/retry`, `/btw`) run in the background so
        // the TUI stays live; every other slash command runs inline. Leading and
        // trailing whitespace is tolerated and must not flip the routing.
        let cases = [
            ("hello", TurnKind::Background),
            ("  do it ", TurnKind::Background),
            ("/retry", TurnKind::Background),
            ("/btw why", TurnKind::Background),
            ("  /retry  ", TurnKind::Background),
            ("/login", TurnKind::Foreground),
            ("/help", TurnKind::Foreground),
            ("/model x", TurnKind::Foreground),
            ("/settings", TurnKind::Foreground),
        ];
        for (input, expected) in cases {
            assert_eq!(default_turn_kind(input), expected, "routing for {input:?}");
        }
    }

    #[test]
    fn spinner_glyph_wraps_modulo_the_frame_count() {
        // The braille cycle has ten frames; frame 10 must wrap back to frame 0
        // rather than indexing past the array (which would panic).
        assert_eq!(spinner_glyph(0), spinner_glyph(10));
    }

    #[test]
    fn spinner_glyph_exposes_ten_distinct_frames() {
        // One full cycle (frames 0..10) must show every glyph exactly once; a
        // duplicate entry or a wrong index would stall or skip in the animation.
        let glyphs: std::collections::HashSet<char> = (0..10).map(spinner_glyph).collect();
        assert_eq!(glyphs.len(), 10, "expected ten distinct spinner frames, got {glyphs:?}");
    }

    #[test]
    fn push_turn_result_maps_ok_slash_prompt_to_a_system_message() {
        // A successful slash command surfaces as a system line.
        let mut messages = Vec::new();
        push_turn_result(&mut messages, "/help", Ok("help text".to_string()));
        assert_eq!(messages.len(), 1, "exactly one message must be pushed");
        assert_eq!(messages[0].role, "system");
    }

    #[test]
    fn push_turn_result_maps_ok_plain_prompt_to_an_assistant_message() {
        // A successful plain prompt surfaces as an assistant reply.
        let mut messages = Vec::new();
        push_turn_result(&mut messages, "hello", Ok("reply".to_string()));
        assert_eq!(messages.len(), 1, "exactly one message must be pushed");
        assert_eq!(messages[0].role, "assistant");
    }

    #[test]
    fn push_turn_result_maps_err_to_an_error_message() {
        // Any failure surfaces as an error line regardless of the prompt.
        let mut messages = Vec::new();
        push_turn_result(&mut messages, "hello", Err("boom".to_string()));
        assert_eq!(messages.len(), 1, "exactly one message must be pushed");
        assert_eq!(messages[0].role, "error");
    }

    #[test]
    fn push_turn_result_trims_surrounding_whitespace_from_ok_text() {
        // Ok text is trimmed before it reaches the transcript so stray agent
        // padding never widens the rendered row.
        let mut messages = Vec::new();
        push_turn_result(&mut messages, "hello", Ok("  hi  ".to_string()));
        assert_eq!(messages[0].text, "hi");
    }
}
