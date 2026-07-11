use std::io::{self, IsTerminal};
use std::sync::atomic::AtomicBool;
use std::sync::Arc;

mod attachments;
mod editor;
mod integration;
mod queue;
mod theme;

mod render;
mod repl;
mod text;
mod view;
mod view_render;

pub(super) use attachments::{AttachmentTray, ClipboardContent};
pub(super) use integration::{RegistryUiRuntime, UiFeature, UiRuntimeAdapter};
pub(super) use queue::{DeliveryAction, FollowUpQueue};

pub(super) use editor::{
    EditorAction, EditorState, EDITOR_KEYMAP_NAMESPACE, EXTERNAL_EDITOR_ACTION_ID,
};

pub use view::{
    CommandOutcome, ConfirmEvent, ConfirmState, PickerEvent, PickerItem, PickerSpec, PickerState,
};

pub use repl::loops::run_basic_loop;

pub(crate) fn external_editor_capability_descriptor(
    cwd: &std::path::Path,
) -> crate::capability::CapabilityDescriptor {
    use crate::capability::{
        CapabilityDescriptor, CapabilityHealth, CapabilityKind, FunctionTarget,
    };

    let health = repl::external_editor::external_editor_health(cwd);
    let mut descriptor = CapabilityDescriptor::new(
        "view/external-editor",
        CapabilityKind::View,
        "jeden-core",
        "External editor",
        "Edit the current prompt with VISUAL or EDITOR",
        FunctionTarget::NativeView {
            command: "external-editor".into(),
        },
    )
    .operation("external-editor")
    .metadata(serde_json::json!({"keymapNamespace": EDITOR_KEYMAP_NAMESPACE}));
    match health {
        Ok(()) => descriptor = descriptor.executable(EXTERNAL_EDITOR_ACTION_ID),
        Err(detail) => {
            descriptor.ui.visible = false;
            descriptor = descriptor.health(CapabilityHealth::unavailable(detail));
        }
    }
    descriptor
}

#[allow(dead_code)]
pub fn render_terminal_frame(options: &FrameOptions) -> String {
    let _capabilities = crate::capability::for_cwd(std::path::Path::new(&options.status.cwd));
    render::render_terminal_frame(options)
}

const PRODUCT: &str = "jeden.";
const VERSION: &str = "v0.1.0";
const ASSISTANT_TITLE: &str = "jeden.";

#[derive(Debug, Clone)]
pub struct Message {
    pub role: String,
    pub text: String,
}

impl Message {
    pub fn new(role: impl Into<String>, text: impl Into<String>) -> Self {
        Self {
            role: role.into(),
            text: text.into(),
        }
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
    std::env::var("COLUMNS")
        .ok()
        .and_then(|value| value.parse().ok())
        .unwrap_or(100)
}

pub fn default_rows() -> usize {
    std::env::var("LINES")
        .or_else(|_| std::env::var("ROWS"))
        .ok()
        .and_then(|value| value.parse().ok())
        .unwrap_or(30)
}

pub fn stdout_supports_color() -> bool {
    io::stdout().is_terminal() && std::env::var_os("NO_COLOR").is_none()
}

/// How a submitted line should run relative to the terminal.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TurnKind {
    /// Runs inline on the main thread with raw mode suspended for commands
    /// that need the cooked terminal or return instantly.
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
    /// True when a picker selection submitted this command; prevents reopening
    /// the same empty-command view for parameterless actions.
    pub from_view: bool,
    /// Live status sink rendered next to the spinner.
    pub progress: &'a dyn Fn(&str),
    /// Per-token streaming sink for live assistant text.
    pub stream: &'a dyn Fn(&str),
    /// Ask a question while the terminal event loop owns stdin.
    pub ask_user: Option<&'a dyn Fn(&str, &[String]) -> Result<String, String>>,
    /// Ask the user to approve a gated tool; returns true to allow.
    pub approve: &'a dyn Fn(&str, &str) -> bool,
}
