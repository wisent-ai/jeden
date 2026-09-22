//! The reviewer that reads a turn after the model produced it.
//!
//! Split out of `slash/modes/mod.rs`, which had grown past the module line cap.

use crate::slash::common::split_head;
use crate::slash::state::{AdvisorState, ModeState};
use crate::slash::SlashContext;
use super::super::current_model_route;

pub(super) fn advisor_model_label(advisor: &AdvisorState, context: &SlashContext<'_>) -> String {
    if advisor.model.is_empty() {
        current_model_route(context)
    } else {
        advisor.model.clone()
    }
}

fn format_advisor_status(advisor: &AdvisorState, context: &SlashContext<'_>) -> String {
    [
        format!(
            "Advisor reviewer is {}.",
            if advisor.enabled {
                "enabled"
            } else {
                "disabled"
            }
        ),
        "Review backend: second model-router call after each successful agent result.".to_string(),
        format!(
            "Configured reviewer route: {}.",
            advisor_model_label(advisor, context)
        ),
        if advisor.last_review.is_some() {
            "Last advisor notes are available with /advisor dump.".to_string()
        } else {
            "No advisor notes have been recorded yet.".to_string()
        },
    ]
    .join("\n")
}

pub(crate) fn handle_advisor(
    args: &str,
    state: &mut ModeState,
    context: &SlashContext<'_>,
) -> Result<String, String> {
    let (head, rest) = split_head(args);
    let verb = if head.is_empty() {
        "status".to_string()
    } else {
        head.to_ascii_lowercase()
    };
    match verb.as_str() {
        "on" => {
            state.advisor.enabled = true;
            Ok(format!("Advisor reviewer enabled.\n{}", format_advisor_status(&state.advisor, context)))
        },
        "off" => {
            state.advisor.enabled = false;
            Ok("Advisor reviewer disabled.".into())
        },
        "status" => Ok(format_advisor_status(&state.advisor, context)),
        "dump" => {
            let Some(review) = &state.advisor.last_review else { return Err("No advisor notes are available yet. Enable /advisor and complete an agent turn first.".into()); };
            if rest.trim().eq_ignore_ascii_case("raw") { return serde_json::to_string_pretty(review).map_err(|e| e.to_string()); }
            Ok(review.get("text").and_then(Value::as_str).unwrap_or("Advisor review is empty.").to_string())
        },
        "configure" => {
            let config_text = rest.trim();
            if config_text.is_empty() { return Ok(format_advisor_status(&state.advisor, context)); }
            let (key, value_rest) = split_head(config_text);
            let mut model = config_text.to_string();
            if key.eq_ignore_ascii_case("model") { model = value_rest.trim().to_string(); }
            else if let Some((left, right)) = key.split_once('=') {
                if left.eq_ignore_ascii_case("model") { model = right.to_string(); }
            }
            if model.is_empty() { return Err("Usage: /advisor configure [model <route>|model=<route>|<route>]".into()); }
            state.advisor.model = model;
            Ok(format!("Advisor reviewer route set to {}.\n{}", state.advisor.model, format_advisor_status(&state.advisor, context)))
        },
        _ => Err("Usage: /advisor [on|off|status|dump [raw]|configure [model <route>|model=<route>|<route>]]".into()),
    }
}
