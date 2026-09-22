//! The objective a session is working towards.
//!
//! Split out of `slash/modes/mod.rs`, which had grown past the module line cap.

use crate::slash::common::split_head;
use crate::slash::state::{GoalState, ModeState};

pub(super) fn format_goal_status(goal: &GoalState) -> String {
    let auto = if goal.auto { "on" } else { "off" };
    if goal.objective.is_empty() {
        return format!(
            "Goal mode has no objective. Use /goal set <objective>.\nAuto lifecycle (oko): {}",
            auto
        );
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
        "Goal mode: {}\nObjective: {}\nBudget: {}\nAuto lifecycle (oko): {}",
        state, goal.objective, budget, auto
    )
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
        "auto" => match rest.trim().to_ascii_lowercase().as_str() {
            "on" => {
                state.goal.auto = true;
                Ok("Goal auto lifecycle enabled: Oko's lifecycle model may start and finish goals from your prompts.".into())
            }
            "off" => {
                state.goal.auto = false;
                Ok("Goal auto lifecycle disabled.".into())
            }
            _ => Err("Usage: /goal auto <on|off>".into()),
        },
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

pub(crate) fn handle_guided_goal(args: &str, state: &mut ModeState) -> Result<String, String> {
    let objective = args.trim();
    if objective.is_empty() {
        return Err("Usage: /guided-goal <rough objective>".into());
    }
    state.guided_goal.active = true;
    state.guided_goal.rough_objective = objective.to_string();
    Ok("Guided goal drafting started. Jeden will use the next turn to refine the objective instead of pretending to open an overlay.".into())
}
