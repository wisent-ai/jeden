use serde_json::Value;
use std::env;

use crate::slash::common::{now_millis, parse_duration_ms, split_args, split_head};
use crate::slash::state::{AdvisorState, ForceState, GoalState, LoopState, ModeState, ToolsState};
use crate::slash::SlashContext;
use crate::tools;

pub(crate) mod session;
pub(crate) mod todo;

fn format_goal_status(goal: &GoalState) -> String {
    if goal.objective.is_empty() {
        return "Goal mode has no objective. Use /goal set <objective>.".into();
    }
    let state = if goal.enabled {
        if goal.paused {
            "paused"
        } else {
            "active"
        }
    } else {
        "disabled"
    };
    let budget = goal
        .budget
        .map(|v| {
            if v.fract() == f64::default() {
                format!("{}", v as i64)
            } else {
                v.to_string()
            }
        })
        .unwrap_or_else(|| "off".into());
    format!(
        "Goal mode: {}\nObjective: {}\nBudget: {}",
        state, goal.objective, budget
    )
}

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

fn advisor_model_label(advisor: &AdvisorState, context: &SlashContext<'_>) -> String {
    if advisor.model.is_empty() {
        current_model_route(context)
    } else {
        advisor.model.clone()
    }
}

fn valid_approval_mode(value: &str) -> bool {
    matches!(value, "always-ask" | "write" | "yolo")
}

fn valid_approval_policy(value: &str) -> bool {
    matches!(value, "allow" | "deny" | "prompt")
}

pub(crate) fn handle_approval(args: &str, state: &mut ModeState) -> Result<String, String> {
    let argv = split_args(args);
    if argv.is_empty() || argv.first().map(String::as_str) == Some("status") {
        let mode = if state.tools.approval_mode.trim().is_empty() {
            "default (safe always-ask unless --yolo or both allow flags are set)"
        } else {
            state.tools.approval_mode.as_str()
        };
        let mut lines = vec![format!("Approval mode: {}", mode)];
        if state.tools.approval.is_empty() {
            lines.push("Per-tool policies: none.".into());
        } else {
            lines.push("Per-tool policies:".into());
            for (tool, policy) in &state.tools.approval {
                lines.push(format!("- {}: {}", tool, policy));
            }
        }
        return Ok(lines.join("\n"));
    }
    let (first, rest) = argv
        .split_first()
        .expect("argv is non-empty after the is_empty guard above");
    match first.as_str() {
        "mode" => {
            let mode = rest.first().map(String::as_str).unwrap_or("");
            if !valid_approval_mode(mode) {
                return Err("Usage: /approval mode <always-ask|write|yolo>".into());
            }
            state.tools.approval_mode = mode.to_string();
            Ok(format!("Approval mode set to {}.", mode))
        }
        "reset" => {
            state.tools = ToolsState::default();
            Ok("Approval policy reset.".into())
        }
        tool => {
            let policy = rest.first().map(String::as_str).unwrap_or("");
            if tool.is_empty() || !valid_approval_policy(policy) {
                return Err("Usage: /approval [status] | mode <always-ask|write|yolo> | <tool> <allow|deny|prompt> | reset".into());
            }
            state
                .tools
                .approval
                .insert(tool.to_string(), policy.to_string());
            Ok(format!("Approval policy for {} set to {}.", tool, policy))
        }
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

pub(crate) fn handle_goal(args: &str, state: &mut ModeState) -> Result<String, String> {
    let (head, rest) = split_head(args);
    let verb = head.to_ascii_lowercase();
    if args.trim().is_empty() || verb == "show" || verb == "status" {
        return Ok(format_goal_status(&state.goal));
    }
    match verb.as_str() {
        "set" => {
            if rest.is_empty() {
                return Err("Usage: /goal set <objective>".into());
            }
            state.goal.objective = rest.to_string();
            state.goal.enabled = true;
            state.goal.paused = false;
            Ok(format!(
                "Goal mode enabled.\nObjective: {}",
                state.goal.objective
            ))
        }
        "pause" => {
            state.goal.paused = true;
            Ok("Goal mode paused.".into())
        }
        "resume" => {
            if state.goal.objective.is_empty() {
                return Err("No goal objective is set. Use /goal set <objective>.".into());
            }
            state.goal.enabled = true;
            state.goal.paused = false;
            Ok("Goal mode resumed.".into())
        }
        "drop" | "off" => {
            state.goal.enabled = false;
            state.goal.paused = false;
            state.goal.objective.clear();
            state.goal.budget = None;
            Ok("Goal mode dropped.".into())
        }
        "budget" => {
            let budget = rest.trim().to_ascii_lowercase();
            if budget.is_empty() || budget == "off" {
                state.goal.budget = None;
                return Ok("Goal budget disabled.".into());
            }
            let parsed = budget
                .parse::<f64>()
                .map_err(|_| "Usage: /goal budget <positive-number|off>".to_string())?;
            if !parsed.is_finite() || parsed <= f64::default() {
                return Err("Usage: /goal budget <positive-number|off>".into());
            }
            state.goal.budget = Some(parsed);
            Ok(format!(
                "Goal budget set to {}.",
                if parsed.fract() == f64::default() {
                    format!("{}", parsed as i64)
                } else {
                    parsed.to_string()
                }
            ))
        }
        _ => {
            state.goal.objective = args.trim().to_string();
            state.goal.enabled = true;
            state.goal.paused = false;
            Ok(format!(
                "Goal mode enabled.\nObjective: {}",
                state.goal.objective
            ))
        }
    }
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

pub(crate) fn handle_guided_goal(args: &str, state: &mut ModeState) -> Result<String, String> {
    let objective = args.trim();
    if objective.is_empty() {
        return Err("Usage: /guided-goal <rough objective>".into());
    }
    state.guided_goal.active = true;
    state.guided_goal.rough_objective = objective.to_string();
    Ok("Guided goal drafting started. Jeden will use the next turn to refine the objective instead of pretending to open an overlay.".into())
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
