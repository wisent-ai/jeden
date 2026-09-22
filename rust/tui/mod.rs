use std::io::{self, IsTerminal};
use std::sync::atomic::AtomicBool;
use std::sync::Arc;

mod capabilities;
mod editor;
mod integration;
mod queue;
pub(crate) mod theme;

mod render;
mod repl;
mod view;

/// Terminal QR rendering, under the name every caller uses.
pub use render::qr;

// The editor's own pieces and the view's renderer, under the names the rest
// of this module has always used.
pub(crate) use editor::{attachments, text};
pub(crate) use view::render as view_render;

pub use attachments::{Attachment, AttachmentId, AttachmentKind, AttachmentSource};
pub(super) use attachments::{AttachmentTray, ClipboardContent};
pub(super) use integration::{RegistryUiRuntime, UiFeature, UiRuntimeAdapter};
pub(super) use queue::{DeliveryAction, FollowUpQueue};

pub(super) use editor::{
    EditorAction, EditorState, EDITOR_KEYMAP_NAMESPACE, EXTERNAL_EDITOR_ACTION_ID,
};

pub use view::{
    CommandOutcome, ConfirmEvent, ConfirmState, PickerEvent, PickerFocus, PickerItem, PickerSpec,
    PickerState,
};

pub use repl::loops::run_basic_loop;

// Gallery-facing component renderers (`jeden gallery` dev tool).
pub(crate) use render::{compact_prompt, welcome_panel};
pub(crate) use repl::message_block;
pub(crate) use view_render::{confirm_panel, picker_panel};

pub(crate) use capabilities::{
    attachment_capability_descriptors, external_editor_capability_descriptor,
    keymap_capability_descriptor,
};

#[allow(dead_code)]
pub fn render_terminal_frame(options: &FrameOptions) -> String {
    let _capabilities = crate::capability::for_cwd(std::path::Path::new(&options.status.cwd));
    render::render_terminal_frame(options)
}

const PRODUCT: &str = "Wisent";
const APP: &str = "Agent";
const VERSION: &str = crate::JEDEN_VERSION;
const ASSISTANT_TITLE: &str = "wisent";

#[derive(Debug, Clone)]
pub struct Message {
    pub role: String,
    pub text: String,
    /// Output of a slash command rather than conversation. Views REPLACE
    /// each other in the live region instead of being committed to the
    /// scrollback: every command used to leave another frame behind, so a
    /// session drifted into a wall of stale panels no one reads.
    pub view: bool,
}

impl Message {
    pub fn new(role: impl Into<String>, text: impl Into<String>) -> Self {
        Self {
            role: role.into(),
            text: text.into(),
            view: false,
        }
    }

    pub fn view(role: impl Into<String>, text: impl Into<String>) -> Self {
        Self {
            view: true,
            ..Self::new(role, text)
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
    /// Runs on a worker thread while the TUI stays live (skeleton + Esc-cancel).
    Background,
}

/// Default policy: agent turns (plain prompts, `/retry`, `/btw`) run in the
/// background so the TUI stays live; view commands that fetch from Brama/Weles
/// before rendering join them so the skeleton acts as their loading state
/// instead of freezing the UI (`/login` also needs Esc-cancel for the device
/// poll). Local, instant commands run inline in the cooked terminal.
pub fn default_turn_kind(input: &str) -> TurnKind {
    let trimmed = input.trim();
    if !trimmed.starts_with('/') {
        return TurnKind::Background;
    }
    let command = trimmed.split_whitespace().next().unwrap_or(trimmed);
    match command {
        "/retry" | "/btw" | "/login" | "/logout" | "/model" | "/models" | "/switch" | "/usage"
        | "/providers" => TurnKind::Background,
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
    /// Immutable typed attachments captured for this turn. Payload bytes remain
    /// shared through `Arc` handles until provider-bound serialization.
    pub attachments: &'a [Attachment],
    /// Live status sink rendered beside the skeleton.
    pub progress: &'a dyn Fn(&str),
    /// Per-token streaming sink for live assistant text.
    pub stream: &'a dyn Fn(&str),
    /// Tool calls, tool results, and reasoning the handler decided to show.
    pub(crate) trace: &'a dyn Fn(&crate::agent::TraceEvent<'_>),
    /// Ask a question while the terminal event loop owns stdin.
    pub ask_user: Option<crate::tool_runtime::AskUserFn<'a>>,
    /// Ask the user to approve a gated tool; returns true to allow.
    pub approve: &'a dyn Fn(&str, &str) -> bool,
}

#[cfg(test)]
mod turn_kind_tests {
    use super::{default_turn_kind, TurnKind};

    #[test]
    fn login_with_provider_runs_in_background() {
        assert_eq!(
            default_turn_kind("/login claude-code"),
            TurnKind::Background
        );
        assert_eq!(default_turn_kind("/login"), TurnKind::Background);
    }

    #[test]
    fn network_views_run_in_background() {
        for input in [
            "/model",
            "/model --all",
            "/models",
            "/usage",
            "/providers",
            "/logout kimi",
        ] {
            assert_eq!(default_turn_kind(input), TurnKind::Background, "{input}");
        }
    }

    #[test]
    fn local_commands_stay_foreground() {
        assert_eq!(default_turn_kind("/settings"), TurnKind::Foreground);
        assert_eq!(default_turn_kind("/help"), TurnKind::Foreground);
    }

    #[test]
    fn agent_turns_run_in_background() {
        assert_eq!(default_turn_kind("fix the bug"), TurnKind::Background);
        assert_eq!(default_turn_kind("/retry"), TurnKind::Background);
    }
}
