use std::io::{self, IsTerminal, Write};
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
        (_, Some(limit)) => format!(" > ◫ n/a/{}", limit),
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
    vec![top, format!("{} {}", paint("╰─", "cyan", color), input_text)]
}

pub fn render_terminal_frame(options: &FrameOptions) -> String {
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
    let mut lines = vec!["\x1b[2J\x1b[H".to_string()];
    lines.extend(main_lines.iter().cloned());
    lines.extend(std::iter::repeat(String::new()).take(available_rows.saturating_sub(main_lines.len())));
    lines.extend(slash_hints);
    lines.extend(prompt);
    lines.join("\n")
}

pub fn render_to_stdout(options: &FrameOptions) -> io::Result<()> {
    let mut stdout = io::stdout();
    stdout.write_all(render_terminal_frame(options).as_bytes())?;
    stdout.write_all(b"\x1b[?25h")?;
    stdout.flush()
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

fn old_read_line_loop<S, F>(mut status_provider: S, mut handle_prompt: F) -> io::Result<()>
where
    S: FnMut() -> PromptStatus,
    F: FnMut(&str) -> Result<String, String>,
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
        match handle_prompt(prompt) {
            Ok(text) => messages.push(Message::new(if prompt.starts_with('/') { "system" } else { "assistant" }, text.trim().to_string())),
            Err(error) => messages.push(Message::new("error", error)),
        }
    }
    Ok(())
}

pub fn run_basic_loop<S, F>(mut status_provider: S, mut handle_prompt: F) -> io::Result<()>
where
    S: FnMut() -> PromptStatus,
    F: FnMut(&str) -> Result<String, String>,
{
    if !io::stdin().is_terminal() {
        return old_read_line_loop(status_provider, handle_prompt);
    }

    let _raw = RawModeGuard::enter()?;
    let mut messages = Vec::new();
    let mut input = String::new();
    let mut slash_selection = 0usize;
    loop {
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
        render_to_stdout(&options)?;

        if !event::poll(Duration::from_millis(250))? {
            continue;
        }
        let Event::Key(key) = event::read()? else {
            continue;
        };
        if !matches!(key.kind, KeyEventKind::Press | KeyEventKind::Repeat) {
            continue;
        }
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
            KeyCode::Enter | KeyCode::Char('\r') | KeyCode::Char('\n') | KeyCode::Char('m') | KeyCode::Char('j')
                if matches!(key.code, KeyCode::Enter | KeyCode::Char('\r') | KeyCode::Char('\n'))
                    || key.modifiers.contains(KeyModifiers::CONTROL) => {
                if input.trim().is_empty() {
                    continue;
                }
                let prompt = input.trim().to_string();
                input.clear();
                slash_selection = 0;
                if matches!(prompt.as_str(), "/exit" | "/quit") {
                    break;
                }
                disable_raw_mode()?;
                let result = handle_prompt(&prompt);
                enable_raw_mode()?;
                messages.push(Message::new("user", prompt.clone()));
                match result {
                    Ok(text) => messages.push(Message::new(if prompt.starts_with('/') { "system" } else { "assistant" }, text.trim().to_string())),
                    Err(error) => messages.push(Message::new("error", error)),
                }
            }
            KeyCode::Char(ch) => {
                input.push(ch);
                slash_selection = 0;
            }
            KeyCode::Up => {
                let count = slash_matches(&input).len();
                if count > 0 {
                    slash_selection = if slash_selection == 0 { count - 1 } else { slash_selection - 1 };
                }
            }
            KeyCode::Down => {
                let count = slash_matches(&input).len();
                if count > 0 {
                    slash_selection = (slash_selection + 1) % count;
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
}
