use std::io::{self, IsTerminal};
use std::sync::atomic::AtomicBool;
use std::sync::Arc;

mod render;
mod repl;
mod text;

pub use repl::loops::run_basic_loop;

#[allow(dead_code)]
pub fn render_terminal_frame(options: &FrameOptions) -> String {
    render::render_terminal_frame(options)
}

pub fn render_to_stdout(options: &FrameOptions) -> io::Result<()> {
    render::render_to_stdout(options)
}


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
    ("approval", "Configure tool approval policy"),
    ("tools", "Show tools"), ("context", "Show context usage"), ("extensions", "Manage extensions"),
    ("agents", "Agent controls"), ("branch", "Create branch"), ("fork", "Create fork"), ("tree", "Navigate tree"),
    ("ssh", "Manage SSH hosts"), ("new", "Start new session"), ("fresh", "Reset provider stream state"),
    ("drop", "Drop current session"), ("compact", "Compact session"), ("shake", "Shake session context"),
    ("handoff", "Hand off session"), ("resume", "Resume session"), ("btw", "Side question"),
    ("tan", "Background agent"), ("omfg", "Forge local rule"), ("retry", "Retry last failed turn"),
    ("debug", "Open debug tools"), ("memory", "Memory maintenance"), ("rename", "Rename session"),
    ("move", "Move session workspace"), ("marketplace", "Manage marketplace plugins"),
    ("plugins", "Manage installed plugins"), ("reload-plugins", "Reload plugins"), ("hooks", "Show lifecycle hooks"),
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
    /// Ask the user to approve a gated tool; returns true to allow.
    pub approve: &'a dyn Fn(&str, &str) -> bool,
}
