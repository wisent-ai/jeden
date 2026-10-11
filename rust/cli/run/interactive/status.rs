//! The status line the terminal draws every frame, built without ever
//! blocking the frame.
//!
//! Split out of `cli/run/interactive.rs`, which had grown past the module line
//! cap.

use serde_json::Value;
use std::env;
use std::path::Path;
use std::process::Command;

pub(super) fn git_prompt_status(cwd: &Path) -> (Option<String>, usize) {
    let branch = Command::new("git")
        .arg("-C")
        .arg(cwd)
        .args(["branch", "--show-current"])
        .output()
        .ok()
        .filter(|output| output.status.success())
        .map(|output| String::from_utf8_lossy(&output.stdout).trim().to_string())
        .filter(|value| !value.is_empty());
    let dirty_count = Command::new("git")
        .arg("-C")
        .arg(cwd)
        .args(["status", "--porcelain"])
        .output()
        .ok()
        .filter(|output| output.status.success())
        .map(|output| String::from_utf8_lossy(&output.stdout).lines().count())
        .unwrap_or_default();
    (branch, dirty_count)
}

/// The service tier the status line shows. A mode state that cannot be read
/// or parsed is named on the line itself: read as empty it showed the
/// environment's tier while the session's own fast mode was in force.
pub(super) fn service_tier_prompt(cwd: &Path) -> String {
    let mode = match crate::slash::state::read_mode_state_value(cwd) {
        Ok(mode) => mode,
        Err(error) => return format!("mode state unreadable: {error}"),
    };
    if mode
        .pointer("/fast/enabled")
        .and_then(Value::as_bool)
        .unwrap_or(false)
    {
        if let Some(tier) = mode
            .pointer("/fast/serviceTier")
            .and_then(Value::as_str)
            .filter(|value| !value.trim().is_empty())
        {
            return tier.to_string();
        }
    }
    env::var("JEDEN_SERVICE_TIER")
        .ok()
        .or_else(|| env::var("MODEL_SERVICE_TIER").ok())
        .filter(|value| !value.trim().is_empty())
        .unwrap_or_else(|| "default".into())
}

/// `JEDEN_CONTEXT_LIMIT`, when it names a whole number of tokens above zero.
/// Read by the session setup for the status line and by `/context`, which the
/// split moved into turn.rs, away from the local both used to share.
pub(super) fn context_limit() -> Option<usize> {
    env::var("JEDEN_CONTEXT_LIMIT")
        .ok()
        .and_then(|v| v.trim().parse::<usize>().ok())
        .filter(|v| *v != usize::default())
}
