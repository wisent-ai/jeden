//! Turning the options a session was opened with into the arguments one run
//! actually takes.
//!
//! Split out of `sdk/session/turn.rs`, which had grown past the module line
//! cap.

use super::super::super::SessionOptions;
use crate::Args;

pub(super) fn args_from_options(options: &SessionOptions, prompt: String, goal: Option<String>) -> Args {
    Args {
        command: "run".into(),
        cwd: options.cwd.clone(),
        cwd_explicit: true,
        model: options.model.clone(),
        max_tokens: options.max_tokens,
        max_steps: options.max_steps,
        allow_write: options.allow_write || options.auto_approve,
        allow_command: options.allow_command || options.auto_approve,
        yolo: options.auto_approve,
        model_only: false,
        json: false,
        resume_session: None,
        autonomous: false,
        pursuit_request: None,
        goal,
        positionals: vec![prompt],
    }
}
