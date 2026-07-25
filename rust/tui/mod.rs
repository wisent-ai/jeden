use std::io::{self, IsTerminal};
use std::sync::atomic::AtomicBool;
use std::sync::Arc;

mod attachments;
mod editor;
mod integration;
mod queue;
pub(crate) mod theme;

mod render;
mod repl;
mod text;
mod view;
mod view_render;

pub use attachments::{Attachment, AttachmentId, AttachmentKind, AttachmentSource};
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

fn key_chord(
    code: &crossterm::event::KeyCode,
    modifiers: crossterm::event::KeyModifiers,
) -> String {
    use crossterm::event::{KeyCode, KeyModifiers};

    let normalized = modifiers
        & (KeyModifiers::SHIFT | KeyModifiers::CONTROL | KeyModifiers::ALT | KeyModifiers::SUPER);
    let mut parts = Vec::with_capacity(5);
    if normalized.contains(KeyModifiers::CONTROL) {
        parts.push("ctrl".to_string());
    }
    if normalized.contains(KeyModifiers::ALT) {
        parts.push("alt".to_string());
    }
    if normalized.contains(KeyModifiers::SHIFT) {
        parts.push("shift".to_string());
    }
    if normalized.contains(KeyModifiers::SUPER) {
        parts.push("super".to_string());
    }
    let key = match code {
        KeyCode::Backspace => "backspace".to_string(),
        KeyCode::Enter => "enter".to_string(),
        KeyCode::Left => "left".to_string(),
        KeyCode::Right => "right".to_string(),
        KeyCode::Up => "up".to_string(),
        KeyCode::Down => "down".to_string(),
        KeyCode::Home => "home".to_string(),
        KeyCode::End => "end".to_string(),
        KeyCode::PageUp => "page-up".to_string(),
        KeyCode::PageDown => "page-down".to_string(),
        KeyCode::Tab => "tab".to_string(),
        KeyCode::BackTab => "back-tab".to_string(),
        KeyCode::Delete => "delete".to_string(),
        KeyCode::Insert => "insert".to_string(),
        KeyCode::F(number) => format!("f{number}"),
        KeyCode::Char(character) => character.to_string(),
        KeyCode::Null => "null".to_string(),
        KeyCode::Esc => "esc".to_string(),
        other => format!("{other:?}").to_ascii_lowercase(),
    };
    parts.push(key);
    parts.join("+")
}

pub(crate) fn keymap_capability_descriptor() -> crate::capability::CapabilityDescriptor {
    use crate::capability::{
        CapabilityDescriptor, CapabilityHealth, CapabilityKind, FunctionTarget, HealthState,
    };
    use std::collections::BTreeMap;

    const IDLE: &str = "idle";
    const BUSY: &str = "busy";
    let editor = editor::ActionKeyMap::default();
    let delivery = queue::DeliveryKeyMap::default();
    let mut bindings = Vec::with_capacity(editor.bindings().len() + delivery.bindings().len());
    let mut by_context_and_chord: BTreeMap<(&'static str, String), Vec<&'static str>> =
        BTreeMap::new();

    for binding in editor.bindings() {
        let chord = key_chord(&binding.code, binding.modifiers);
        let action_id = binding.action.action_id();
        bindings.push(serde_json::json!({
            "namespace": EDITOR_KEYMAP_NAMESPACE,
            "context": IDLE,
            "chord": chord.clone(),
            "actionId": action_id,
        }));
        by_context_and_chord
            .entry((IDLE, chord))
            .or_default()
            .push(action_id);
    }
    for binding in delivery.bindings() {
        let chord = key_chord(&binding.code, binding.modifiers);
        let action_id = binding.action.action_id();
        bindings.push(serde_json::json!({
            "namespace": queue::DELIVERY_KEYMAP_NAMESPACE,
            "context": BUSY,
            "chord": chord.clone(),
            "actionId": action_id,
        }));
        by_context_and_chord
            .entry((BUSY, chord))
            .or_default()
            .push(action_id);
    }

    let mut conflicts = Vec::new();
    for ((context, chord), mut action_ids) in by_context_and_chord {
        action_ids.sort_unstable();
        action_ids.dedup();
        if action_ids.len() > 1 {
            conflicts.push(serde_json::json!({
                "context": context,
                "chord": chord,
                "actionIds": action_ids,
            }));
        }
    }
    let conflict_count = conflicts.len();
    let mut descriptor = CapabilityDescriptor::new(
        "service/tui-keymap",
        CapabilityKind::Service,
        "jeden-core",
        "TUI keymap",
        "Namespaced editor and delivery key bindings with context-aware conflict diagnostics",
        FunctionTarget::Service {
            name: "tui-keymap".into(),
        },
    )
    .operation("inspect-keymap")
    .operation("diagnose-keymap-conflicts")
    .metadata(serde_json::json!({
        "keymapVersion": 1,
        "contexts": [IDLE, BUSY],
        "contextsMutuallyExclusive": true,
        "bindings": bindings,
        "conflicts": conflicts,
    }));
    if conflict_count > 0 {
        descriptor = descriptor.health(CapabilityHealth {
            state: HealthState::Degraded,
            detail: Some(format!(
                "{conflict_count} active-context keymap conflict(s)"
            )),
        });
    }
    descriptor
}
pub(crate) fn attachment_capability_descriptors(
    cwd: &std::path::Path,
) -> Vec<crate::capability::CapabilityDescriptor> {
    use crate::capability::{
        CapabilityDescriptor, CapabilityHealth, CapabilityKind, FunctionTarget,
    };

    let interactive = io::stdin().is_terminal() && io::stdout().is_terminal() && cwd.is_dir();
    [
        (
            "attach",
            "Attach file",
            "Add a jailed image or small UTF-8 file to the next TUI turn",
        ),
        (
            "attachments",
            "List attachments",
            "List files pending in the interactive TUI attachment tray",
        ),
        (
            "detach",
            "Detach file",
            "Remove one or all files from the interactive TUI attachment tray",
        ),
    ]
    .into_iter()
    .map(|(command, label, description)| {
        let mut descriptor = CapabilityDescriptor::new(
            format!("slash/{command}"),
            CapabilityKind::SlashCommand,
            "jeden-tui",
            label,
            description,
            FunctionTarget::NativeView {
                command: command.into(),
            },
        )
        .operation("attachment-tray")
        .metadata(serde_json::json!({"runtime": "interactive-tui"}));
        if interactive {
            descriptor = descriptor.executable(format!("/{command}"));
        } else {
            descriptor.ui.visible = false;
            descriptor = descriptor.health(CapabilityHealth::unavailable(
                "attachment tray commands require an interactive stdin/stdout TTY",
            ));
        }
        descriptor
    })
    .collect()
}

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
    /// Immutable typed attachments captured for this turn. Payload bytes remain
    /// shared through `Arc` handles until provider-bound serialization.
    pub attachments: &'a [Attachment],
    /// Live status sink rendered next to the spinner.
    pub progress: &'a dyn Fn(&str),
    /// Per-token streaming sink for live assistant text.
    pub stream: &'a dyn Fn(&str),
    /// Ask a question while the terminal event loop owns stdin.
    pub ask_user: Option<&'a dyn Fn(&str, &[String]) -> Result<String, String>>,
    /// Ask the user to approve a gated tool; returns true to allow.
    pub approve: &'a dyn Fn(&str, &str) -> bool,
}
