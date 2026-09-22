//! The standing shape of a session: what it is working on, how it loops, how
//! fast it answers, and what it may force.

use serde_json::Value;
use std::env;

use crate::slash::common::{now_millis, parse_duration_ms, split_head};
use crate::slash::state::{ForceState, LoopState, ModeState};
use crate::slash::SlashContext;
use crate::tools;

mod commands;
pub(crate) mod session;
pub(crate) mod todo;

use commands::{advisor_model_label, format_goal_status};
pub(crate) use commands::{handle_advisor, handle_approval, handle_goal, handle_guided_goal};

fn format_loop_status(loop_state: &LoopState) -> String {
    if !loop_state.enabled {
        return "Loop mode is disabled.".into();
    }
    let mut limits = Vec::new();
    if let Some(remaining) = loop_state.remaining {
        limits.push(format!("{} resubmission(s) remaining", remaining));
    }
    if let Some(until) = loop_state.until {
        limits.push(format!("until epoch-ms {}", until));
    }
    if !loop_state.prompt.is_empty() {
        limits.push(format!("prompt: {}", loop_state.prompt));
    }
    if limits.is_empty() {
        "Loop mode is enabled.".into()
    } else {
        format!("Loop mode is enabled ({}).", limits.join(", "))
    }
}

pub(crate) fn current_model_route(context: &SlashContext<'_>) -> String {
    context
        .model
        .map(ToString::to_string)
        .or_else(|| env::var("JEDEN_MODEL").ok())
        .or_else(|| env::var("MODEL").ok())
        .unwrap_or_else(|| "default".into())
}


pub(crate) fn handle_plan(args: &str, state: &mut ModeState) -> Result<String, String> {
    let (head, rest) = split_head(args);
    let verb = head.to_ascii_lowercase();
    if args.trim().is_empty() {
        state.plan.enabled = !state.plan.enabled;
        return Ok(format!(
            "Plan mode {}.",
            if state.plan.enabled {
                "enabled"
            } else {
                "disabled"
            }
        ));
    }
    match verb.as_str() {
        "on" => {
            state.plan.enabled = true;
            Ok("Plan mode enabled.".into())
        }
        "off" => {
            state.plan.enabled = false;
            Ok("Plan mode disabled.".into())
        }
        "status" => Ok(format!(
            "Plan mode is {}.{}",
            if state.plan.enabled {
                "enabled"
            } else {
                "disabled"
            },
            if state.plan.latest_plan.is_empty() {
                ""
            } else {
                "\nLatest plan is available for /plan-review."
            }
        )),
        "run" if !rest.is_empty() => {
            state.plan.enabled = true;
            Ok("Plan mode enabled for this prompt.".into())
        }
        _ => {
            state.plan.enabled = true;
            Ok("Plan mode enabled for this prompt.".into())
        }
    }
}

pub(crate) fn handle_plan_review(state: &ModeState) -> Result<String, String> {
    if !state.plan.enabled && state.plan.latest_plan.trim().is_empty() {
        return Ok("Warning: Plan mode is not active.".into());
    }
    if state.plan.latest_plan.trim().is_empty() {
        return Ok("No plan review is available yet.".into());
    }
    Ok(state.plan.latest_plan.clone())
}

pub(crate) fn handle_loop(args: &str, state: &mut ModeState) -> Result<String, String> {
    let (head, rest) = split_head(args);
    let verb = head.to_ascii_lowercase();
    if verb == "off" || verb == "stop" {
        state.loop_mode = LoopState::default();
        return Ok("Loop mode disabled.".into());
    }
    if verb == "status" {
        return Ok(format_loop_status(&state.loop_mode));
    }
    let mut prompt = args.trim();
    state.loop_mode.remaining = None;
    state.loop_mode.until = None;
    if !head.is_empty() && head.chars().all(|ch| ch.is_ascii_digit()) {
        state.loop_mode.remaining = head.parse::<u64>().ok();
        prompt = rest;
    } else if let Some(duration) = parse_duration_ms(head) {
        state.loop_mode.until = Some(now_millis() + duration);
        prompt = rest;
    }
    state.loop_mode.enabled = true;
    state.loop_mode.prompt = prompt.to_string();
    let qualifier = if let Some(remaining) = state.loop_mode.remaining {
        format!(" for {} resubmission(s)", remaining)
    } else if state.loop_mode.until.is_some() {
        " until the duration expires".to_string()
    } else {
        String::new()
    };
    Ok(format!("Loop mode enabled{}.", qualifier))
}

pub(crate) fn handle_fast(args: &str, state: &mut ModeState) -> Result<String, String> {
    let (head, rest) = split_head(args);
    let verb = head.to_ascii_lowercase();
    match verb.as_str() {
        "" => state.fast.enabled = !state.fast.enabled,
        "on" => state.fast.enabled = true,
        "off" => state.fast.enabled = false,
        "tier" => {
            if rest.is_empty() {
                return Err("Usage: /fast tier <service-tier>".into());
            }
            state.fast.service_tier = rest.to_string();
            state.fast.enabled = true;
        }
        "status" => {}
        _ => return Err("Usage: /fast [on|off|status|tier <service-tier>]".into()),
    }
    let tier = if state.fast.service_tier.is_empty() {
        "priority"
    } else {
        &state.fast.service_tier
    };
    Ok(format!(
        "Fast mode is {}. Model-router service_tier for future requests: {}.",
        if state.fast.enabled {
            "enabled"
        } else {
            "disabled"
        },
        if state.fast.enabled {
            tier
        } else {
            "(default)"
        }
    ))
}

pub(crate) fn handle_force(
    args: &str,
    state: &mut ModeState,
    context: &SlashContext<'_>,
) -> Result<String, String> {
    let (tool, _prompt) = split_head(args);
    if tool.is_empty() {
        return Err("Usage: /force <tool-name> [prompt]".into());
    }
    let names = tools::list_tools(context.cwd)
        .into_iter()
        .map(|tool| tool.name)
        .collect::<Vec<_>>();
    if !names.is_empty() && !names.iter().any(|name| name == tool) {
        // The prior visible-tools preview cap was an unconsented numeric literal;
        // list every visible tool instead.
        return Err(format!(
            "Unknown or unavailable tool: {}. Visible tools: {}",
            tool,
            names.join(", ")
        ));
    }
    state.force = Some(ForceState {
        tool: tool.to_string(),
        prompt: String::new(),
    });
    Ok(format!(
        "The next agent turn will be instructed to use {} first.",
        tool
    ))
}

pub(crate) fn handle_branching(
    command: &str,
    _args: &str,
    state: &ModeState,
) -> Result<String, String> {
    if command == "/tree" {
        if state.branches.is_empty() {
            return Ok(
                "No branches yet. Create one in an interactive session with /branch <title>."
                    .into(),
            );
        }
        return Ok(state
            .branches
            .iter()
            .map(|branch| {
                format!(
                    "{}\t{}\t{}\t{}",
                    branch.id, branch.title, branch.created_at, branch.path
                )
            })
            .collect::<Vec<_>>()
            .join("\n"));
    }
    // /branch and /fork need a live conversation to fork; the interactive loop
    // handles them directly. The one-shot CLI has no live conversation.
    Err(format!("{} requires an interactive session (it forks the live conversation). Start `jeden` and run {} there.", command, command))
}
