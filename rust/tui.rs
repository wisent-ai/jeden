use std::io::{self, IsTerminal, Write};

const PRODUCT: &str = "Jeden";
const APP: &str = "Agent";
const VERSION: &str = "v0.1.0";
const ASSISTANT_TITLE: &str = "jeden";


const SLASH_COMMAND_HINTS: &[(&str, &str)] = &[
    ("settings", "Open settings menu"),
    ("setup", "Open provider setup"),
    ("plan", "Toggle plan mode"),
    ("goal", "Toggle goal mode"),
    ("loop", "Toggle loop mode"),
    ("model", "Switch model"),
    ("fast", "Toggle priority service tier"),
    ("advisor", "Toggle advisor reviewer"),
    ("help", "Show slash commands"),
    ("login", "Automated OAuth login"),
    ("logout", "Logout provider"),
    ("usage", "Show provider usage"),
    ("update", "Show update command"),
    ("exit", "Exit"),
    ("quit", "Quit"),
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
    let left = [
        String::new(),
        "Welcome back!".to_string(),
        String::new(),
        "JEDEN".to_string(),
        "Agent CLI".to_string(),
        String::new(),
        String::new(),
        if model.is_empty() { "default".to_string() } else { model.to_string() },
        String::new(),
    ];
    let right = [
        "Tips".to_string(),
        "Type a task and press Enter".to_string(),
        "/help for commands".to_string(),
        "/model to switch routes".to_string(),
        "/update for upgrade steps".to_string(),
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


fn slash_hint_panel(input_text: &str, width: usize, color: bool) -> Vec<String> {
    let text = input_text.trim_start();
    if !text.starts_with('/') || text.contains('\n') {
        return Vec::new();
    }
    let prefix = text.trim_start_matches('/').split_whitespace().next().unwrap_or("").to_ascii_lowercase();
    let rows: Vec<String> = SLASH_COMMAND_HINTS
        .iter()
        .filter(|(name, _)| name.starts_with(&prefix))
        .take(6)
        .map(|(name, description)| format!("/{:<15} — {}", name, description))
        .collect();
    if rows.is_empty() { Vec::new() } else { boxed("slash commands", &rows, width, color) }
}

fn compact_prompt(width: usize, status: &PromptStatus, _busy: bool, color: bool) -> Vec<String> {
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
    vec![top, format!("{} ", paint("╰─", "cyan", color))]
}

pub fn render_terminal_frame(options: &FrameOptions) -> String {
    let width = options.columns.min(120).max(50);
    let prompt = compact_prompt(width, &options.status, options.busy, options.color);
    let slash_hints = slash_hint_panel(&options.input_text, width, options.color);
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

pub fn run_basic_loop<S, F>(mut status_provider: S, mut handle_prompt: F) -> io::Result<()>
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

    let mut stdout = io::stdout();
    stdout.write_all(b"\x1b[?25h\n")?;
    stdout.flush()
}

#[cfg(test)]
mod tests {
    use super::*;

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
        });
        assert!(frame.contains("Jeden Agent v0.1.0"));
        assert!(frame.contains("Tool gates: write ask · command ask"));
        assert!(frame.contains("jeden > ⬢ test-model"));
        assert!(frame.contains("Welcome back!"));
        assert!(frame.contains("/update for upgrade steps"));
        assert!(frame.ends_with("╰─ "));
    }
}
