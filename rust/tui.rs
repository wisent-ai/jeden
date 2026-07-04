use std::io::{self, IsTerminal, Write};

const PRODUCT: &str = "Wisent";
const APP: &str = "Agent";
const VERSION: &str = "v0.1.0";
const ASSISTANT_TITLE: &str = "wisent";
const HEADER_MARK: &str = "◒";
const CURSOR: &str = "▌";

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
pub struct FrameOptions {
    pub cwd: String,
    pub write_status: String,
    pub command_status: String,
    pub model: String,
    pub messages: Vec<Message>,
    pub input_text: String,
    pub cursor_index: usize,
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
    let mark_width = WISENT_MARK.iter().map(|line| visible_len(line)).max().unwrap_or(0);
    let controls = [
        "Controls",
        "# prompt actions   / commands",
        "! shell            $ node/python",
        "Enter sends        Ctrl-J newline",
        "arrows/Home/End edit",
    ];
    let mut rows = vec![
        format!("{} private agent harness", PRODUCT),
        format!("Model route: {}", if model.is_empty() { "default" } else { model }),
        format!("Workspace: {}", cwd),
        String::new(),
    ];
    for (index, line) in WISENT_MARK.iter().enumerate() {
        rows.push(format!("{}   {}", pad_visible(line, mark_width), controls.get(index).copied().unwrap_or("")).trim_end().to_string());
    }
    rows.extend([
        String::new(),
        format!("Tool gates: write {} · command {}", write_status, command_status),
        "Sessions: local history and artifacts".to_string(),
        "MCP: adapters loaded through Wisent registry".to_string(),
    ]);
    boxed(&title, &rows, width, color)
}

fn brand_header(width: usize, model: &str, cwd: &str, color: bool) -> String {
    let inner = width.saturating_sub(4).max(8);
    let content_width = inner.saturating_sub(2).max(1);
    let label = format!("{} {} {} {} · {} · {}", HEADER_MARK, PRODUCT, APP, VERSION, if model.is_empty() { "default" } else { model }, cwd);
    let row = if visible_len(&label) > content_width {
        format!("{}…", take_visible(&label, content_width.saturating_sub(1)))
    } else {
        label
    };
    format!("{} {} {}", paint("╭─", "cyan", color), pad_visible(&row, content_width), paint("─╮", "cyan", color))
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

fn with_cursor_marker(value: &str, cursor_index: usize, color: bool) -> String {
    let chars: Vec<char> = value.chars().collect();
    let index = cursor_index.min(chars.len());
    let before: String = chars[..index].iter().collect();
    let after: String = chars[index..].iter().collect();
    format!("{}{}{}", before, paint(CURSOR, "yellow", color), after)
}

fn compact_prompt(width: usize, model: &str, cwd: &str, write_status: &str, command_status: &str, input_text: &str, cursor_index: usize, busy: bool, color: bool) -> Vec<String> {
    let inner = width.saturating_sub(2).max(48);
    let state = if busy { paint("thinking", "yellow", color) } else { paint("ready", "green", color) };
    let label = format!(" wisent > {} · {} > {} > write {} > command {} › ", if model.is_empty() { "default" } else { model }, state, cwd, write_status, command_status);
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
    let visible_input = if input_text.is_empty() { paint(CURSOR, "yellow", color) } else { with_cursor_marker(input_text, cursor_index, color) };
    let mut input_rows = Vec::new();
    for row in visible_input.split('\n') {
        input_rows.extend(wrap_line(row, inner.saturating_sub(4)));
    }
    let rows: Vec<String> = input_rows.into_iter().take(4).collect();
    let mut out = vec![top];
    for row in rows.iter().take(rows.len().saturating_sub(1)) {
        out.push(format!("{} {} {}", paint("│", "cyan", color), pad_visible(row, inner.saturating_sub(2)), paint("│", "cyan", color)));
    }
    let last = rows.last().cloned().unwrap_or_else(|| paint(CURSOR, "yellow", color));
    out.push(format!("{} {} {}", paint("╰─", "cyan", color), pad_visible(&last, inner.saturating_sub(4)), paint("─╯", "cyan", color)));
    out.push(paint(" Enter sends · Ctrl-J newline · arrows/Home/End edit · ↑/↓ history · Ctrl-C exits", "dim", color));
    out
}

pub fn render_terminal_frame(options: &FrameOptions) -> String {
    let width = options.columns.min(120).max(50);
    let header = brand_header(width, &options.model, &options.cwd, options.color);
    let prompt = compact_prompt(
        width,
        &options.model,
        &options.cwd,
        &options.write_status,
        &options.command_status,
        &options.input_text,
        options.cursor_index,
        options.busy,
        options.color,
    );
    let slash_hints = slash_hint_panel(&options.input_text, width, options.color);
    let reserved = 1 + prompt.len() + slash_hints.len() + 1;
    let available_rows = options.rows.saturating_sub(reserved).max(4);
    let message_lines: Vec<String> = options.messages.iter().flat_map(|message| format_message(message, width, options.color)).collect();
    let mut main_lines = if message_lines.is_empty() {
        welcome_panel(width, &options.model, &options.cwd, &options.write_status, &options.command_status, options.color)
    } else {
        message_lines
    };
    if main_lines.len() > available_rows {
        main_lines = main_lines.split_off(main_lines.len() - available_rows);
    }
    let mut lines = vec!["\x1b[2J\x1b[H\x1b[?25l".to_string(), header];
    lines.extend(main_lines.iter().cloned());
    lines.extend(std::iter::repeat(String::new()).take(available_rows.saturating_sub(main_lines.len())));
    lines.extend(slash_hints);
    lines.extend(prompt);
    lines.push("\x1b[?25h".to_string());
    lines.join("\n")
}

pub fn render_to_stdout(options: &FrameOptions) -> io::Result<()> {
    let mut stdout = io::stdout();
    stdout.write_all(render_terminal_frame(options).as_bytes())?;
    stdout.write_all(b"\n")?;
    stdout.flush()
}

#[derive(Debug, Clone)]
pub struct InteractiveConfig {
    pub cwd: String,
    pub write_status: String,
    pub command_status: String,
    pub model: String,
}

pub fn run_basic_loop<F>(config: InteractiveConfig, mut handle_prompt: F) -> io::Result<()>
where
    F: FnMut(&str) -> Result<String, String>,
{
    let mut messages = Vec::new();
    loop {
        let options = FrameOptions {
            cwd: config.cwd.clone(),
            write_status: config.write_status.clone(),
            command_status: config.command_status.clone(),
            model: config.model.clone(),
            messages: messages.clone(),
            input_text: String::new(),
            cursor_index: 0,
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
            cwd: "/tmp/work".to_string(),
            write_status: "ask".to_string(),
            command_status: "ask".to_string(),
            model: "test-model".to_string(),
            messages: Vec::new(),
            input_text: String::new(),
            cursor_index: 0,
            busy: false,
            columns: 80,
            rows: 24,
            color: false,
        });
        assert!(frame.contains("Wisent Agent v0.1.0"));
        assert!(frame.contains("Tool gates: write ask · command ask"));
        assert!(frame.contains("wisent > test-model"));
    }
}
