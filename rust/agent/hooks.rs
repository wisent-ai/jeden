use super::*;
use crate::tool_runtime::runtime_ops::{ArtifactSink, CancellationToken, OperationContext};
use std::path::Path;
use std::sync::Arc;

#[derive(Debug, Clone)]
pub(crate) struct RunResult {
    pub(crate) text: String,
    pub(crate) session_path: Option<PathBuf>,
}

/// Cooperative controls for a turn: cancellation, live progress, streaming,
/// terminal-owned questions, tool approval, and non-TUI interactivity.
pub(crate) struct RunHooks<'a> {
    pub(crate) cancel: std::sync::Arc<std::sync::atomic::AtomicBool>,
    pub(crate) interactive: bool,
    pub(crate) progress: Box<dyn Fn(&str) + 'a>,
    /// Per-token streaming sink for live assistant text.
    pub(crate) stream: Box<dyn Fn(&str) + 'a>,
    /// Terminal-owned human question channel when a live event loop is present.
    pub(crate) ask_user: Option<Box<dyn Fn(&str, &[String]) -> Result<String, String> + 'a>>,
    /// Ask the user to approve a gated tool that isn't pre-authorized. The
    /// detail string carries safety/approval context for the prompt body.
    pub(crate) approve: Box<dyn Fn(&str, &str) -> bool + 'a>,
}

impl RunHooks<'static> {
    /// Non-interactive-safe default for the CLI `run` path (no TUI, no cancel).
    pub(crate) fn inert() -> Self {
        Self {
            cancel: std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false)),
            interactive: true,
            progress: Box::new(|_| {}),
            stream: Box::new(|_| {}),
            ask_user: None,
            approve: Box::new(|_, _| false),
        }
    }
}

impl RunHooks<'_> {
    pub(super) fn cancelled(&self) -> bool {
        self.cancel.load(std::sync::atomic::Ordering::Relaxed)
    }

    pub(super) fn note(&self, message: &str) {
        (self.progress)(message);
    }

    pub(super) fn push_delta(&self, piece: &str) {
        (self.stream)(piece);
    }

    pub(super) fn approve(&self, tool: &str, detail: &str) -> bool {
        (self.approve)(tool, detail)
    }

    pub(super) fn operation_context<'a>(&'a self, artifact_dir: &Path) -> OperationContext<'a> {
        let progress = &self.progress;
        OperationContext::new(
            CancellationToken::from_flag(Arc::clone(&self.cancel)),
            ArtifactSink::new(artifact_dir),
        )
        .with_progress(Arc::new(move |event| {
            progress(&format!(
                "{}: +{} bytes ({} total)",
                event.stream, event.bytes, event.total_bytes
            ));
        }))
    }
}

/// Tools that mutate the filesystem (require write authorization).
pub(crate) fn is_write_tool(tool: &str) -> bool {
    matches!(
        tool,
        "write_file" | "apply_patch" | "edit_file" | "edit" | "delete_file" | "move_file"
    )
}

/// Tools that execute commands/code (require command authorization).
pub(crate) fn is_command_tool(tool: &str) -> bool {
    matches!(
        tool,
        "run_command"
            | "run_process"
            | "node_eval"
            | "python_eval"
            | "run_package_script"
            | "delegate_task"
    )
}
