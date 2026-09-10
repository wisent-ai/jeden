use crate::slash::common::split_args;
use crate::slash::state::ModeState;
use crate::slash::SlashContext;
use crate::tui::{PickerItem, PickerSpec};

pub(crate) fn plan_picker(state: &ModeState, lang: &str) -> PickerSpec {
    let mut items = vec![PickerItem::action(
        if state.plan.enabled {
            "Disable plan mode"
        } else {
            "Enable plan mode"
        },
        if state.plan.enabled {
            "/plan off"
        } else {
            "/plan on"
        },
    )
    .detail(if state.plan.enabled {
        "Plan mode is currently enabled"
    } else {
        "Plan mode is currently disabled"
    })
    .badge(if state.plan.enabled { "ON" } else { "OFF" })];
    items.push(
        PickerItem::action("Show plan status", "/plan status")
            .detail("Show mode and plan availability"),
    );
    if !state.plan.latest_plan.trim().is_empty() {
        items.push(
            PickerItem::action("Review latest plan", "/plan-review")
                .detail("Open the latest agent plan")
                .badge(crate::cli::i18n::tr(lang, "badge.available")),
        );
    }
    PickerSpec::new("Plan workflow", items)
}

pub(crate) fn goal_picker(state: &ModeState, lang: &str) -> PickerSpec {
    let goal = &state.goal;
    let badge = if goal.objective.trim().is_empty() {
        "NO GOAL"
    } else if !goal.enabled {
        "OFF"
    } else if goal.paused {
        "PAUSED"
    } else {
        crate::cli::i18n::tr(lang, "badge.active")
    };
    let detail = if goal.objective.trim().is_empty() {
        "No objective is configured".to_string()
    } else {
        format!("Objective: {}", goal.objective)
    };
    let mut items = vec![PickerItem::action("Show goal", "/goal status")
        .detail(detail)
        .badge(badge)];
    items.push(
        PickerItem::action(
            if goal.objective.trim().is_empty() {
                "Set goal objective"
            } else {
                "Change goal objective"
            },
            "/goal set ",
        )
        .detail("Edit the objective in the main prompt before submitting")
        .badge("INPUT")
        .prefill(),
    );
    if !goal.objective.trim().is_empty() {
        if goal.enabled && !goal.paused {
            items.push(
                PickerItem::action("Pause goal", "/goal pause")
                    .detail("Keep the objective but stop goal prompting"),
            );
        } else {
            items.push(
                PickerItem::action("Resume goal", "/goal resume")
                    .detail("Continue the configured objective"),
            );
        }
        if let Some(budget) = goal.budget {
            items.push(
                PickerItem::action("Disable goal budget", "/goal budget off")
                    .detail(format!("Current budget: {}", budget))
                    .badge("BUDGET"),
            );
        }
        items.push(
            PickerItem::action("Drop goal", "/goal drop")
                .detail("Clear the objective and budget")
                .badge("DESTRUCTIVE"),
        );
    }
    PickerSpec::new("Goal workflow", items)
}

pub(crate) fn loop_picker(state: &ModeState) -> PickerSpec {
    let mode = &state.loop_mode;
    let detail = if !mode.prompt.trim().is_empty() {
        format!("Prompt: {}", mode.prompt)
    } else if let Some(remaining) = mode.remaining {
        format!("{} resubmission(s) remaining", remaining)
    } else if let Some(until) = mode.until {
        format!("Runs until epoch-ms {}", until)
    } else {
        "No prompt or limit configured".to_string()
    };
    let mut items = vec![PickerItem::action("Show loop status", "/loop status")
        .detail(detail)
        .badge(if mode.enabled { "ON" } else { "OFF" })];
    items.push(if mode.enabled {
        PickerItem::action("Stop loop", "/loop off").detail("Stop automatic resubmission")
    } else {
        PickerItem::action("Start open-ended loop", "/loop")
            .detail("Enable resubmission without a prompt or limit")
    });
    items.push(
        PickerItem::action("Configure loop prompt or limit", "/loop ")
            .detail("Edit an optional count or duration followed by the loop prompt")
            .badge("INPUT")
            .prefill(),
    );
    PickerSpec::new("Loop workflow", items)
}

pub(crate) fn fast_picker(state: &ModeState) -> PickerSpec {
    let tier = if state.fast.service_tier.trim().is_empty() {
        "priority"
    } else {
        state.fast.service_tier.as_str()
    };
    PickerSpec::new(
        "Fast mode",
        vec![
            PickerItem::action(
                if state.fast.enabled {
                    "Disable fast mode"
                } else {
                    "Enable fast mode"
                },
                if state.fast.enabled {
                    "/fast off"
                } else {
                    "/fast on"
                },
            )
            .detail(format!("Configured service tier: {}", tier))
            .badge(if state.fast.enabled { "ON" } else { "OFF" }),
            PickerItem::action("Show fast mode status", "/fast status")
                .detail("Show the model-router service tier"),
        ],
    )
}

pub(crate) fn todo_picker() -> PickerSpec {
    PickerSpec::new(
        "Retained tasks",
        vec![
            PickerItem::action("Show every retained task", "/todo list"),
            PickerItem::action("Continue unfinished work", "/todo continue"),
            PickerItem::action("Add a user request", "/todo add ").prefill(),
            PickerItem::action("Pause a task", "/todo pause <id> --revision <n> --reason ")
                .prefill(),
            PickerItem::action(
                "Resume a task",
                "/todo resume <id> --revision <n> --reason ",
            )
            .prefill(),
            PickerItem::action(
                "Cancel a task",
                "/todo cancel <id> --revision <n> --reason ",
            )
            .prefill(),
        ],
    )
}

pub(crate) fn handle_todo(args: &str, context: &SlashContext<'_>) -> Result<String, String> {
    crate::completion::cli::execute(context.cwd, &split_args(args), false, None)
}
