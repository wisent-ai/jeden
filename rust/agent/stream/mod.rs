//! How long a turn waits for the model's first token.
//!
//! The router's own default gives the whole request thirty seconds to produce
//! the first streamed event, and counts response headers as nothing: the clock
//! runs until an SSE payload parses. That is the wrong question asked of a
//! reasoning route. On 2026-09-15 every assignment on this workstation died at
//! it, three attempts at a time: the acceptance review of session
//! `1789459603-sQaGlP` asked `codex/gpt-6-astra` for a verdict on a transcript,
//! the route needed longer than thirty seconds to start speaking, and the turn
//! recorded
//!
//! ```text
//! retry 1/2 after model stream first-event timeout
//! retry 2/2 after model stream first-event timeout
//! Error: Work remains open (acceptance_review): model stream first-event timeout
//! ```
//!
//! a hundred seconds after the request, reporting work it had already finished
//! as open. The same shape ended six more runs that afternoon. Nothing was
//! wrong with the gateway: two earlier steps of that same session were answered
//! by the same route within ten seconds each, on much smaller prompts.
//!
//! So the bound is raised to the latency of an answer rather than the health of
//! a connection. A gateway that is down still fails at once — a refused
//! connection is a transport error, not this timeout — and a stream that begins
//! and then stops is still cut by `idleTimeoutMs`. An operator who sets
//! `modelRouting.retry.firstEventTimeoutMs` keeps exactly that value.

use std::path::Path;
use std::time::Duration;

use crate::model_router::ChatConfig;

/// The setting an operator writes to choose this themselves.
const FIRST_EVENT_SETTING: &str = "modelRouting.retry.firstEventTimeoutMs";

/// How long a reasoning route may take to start speaking. Five minutes is the
/// measured hundred seconds of that review request with room for a longer
/// prompt, and it is still a bound: the stream has to arrive, and silence after
/// the first token is `idleTimeoutMs`'s business.
const TIME_TO_FIRST_TOKEN: Duration = Duration::from_secs(300);

/// Raise the time-to-first-token bound unless the operator has chosen one.
///
/// `cwd` is the workspace whose merged configuration decides that — the same
/// one the router read its policy from.
pub(crate) fn apply_stream_policy(router: &mut ChatConfig, cwd: &Path) {
    if operator_chose_the_bound(cwd) {
        return;
    }
    if router.retry.first_event_timeout < TIME_TO_FIRST_TOKEN {
        router.retry.first_event_timeout = TIME_TO_FIRST_TOKEN;
    }
}

fn operator_chose_the_bound(cwd: &Path) -> bool {
    let config = crate::cli::config::merged_config_value(cwd);
    crate::cli::config::config_value_at(&config, FIRST_EVENT_SETTING).is_some()
}
