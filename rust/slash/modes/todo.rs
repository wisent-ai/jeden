use crate::slash::common::{now_text, resolve_cwd_path, split_args};
use crate::slash::state::{ModeState, TodoState};
use crate::slash::SlashContext;
use crate::tui::{PickerItem, PickerSpec};
use std::fs;

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

fn quoted_arg(value: &str) -> String {
    format!("\"{}\"", value.replace('\\', "\\\\").replace('\"', "\\\""))
}

pub(crate) fn todo_picker(state: &ModeState, lang: &str) -> PickerSpec {
    let mut items = vec![PickerItem::action("Show todo list", "/todo list")
        .detail(if state.todos.is_empty() {
            "The todo list is empty"
        } else {
            "Show every todo and its status"
        })
        .badge(if state.todos.is_empty() {
            "EMPTY"
        } else {
            crate::cli::i18n::tr(lang, "badge.current")
        })];
    items.push(
        PickerItem::action("Add todo", "/todo add ")
            .detail("Edit the task text in the main prompt before submitting")
            .badge("INPUT")
            .prefill(),
    );
    if !state.todos.is_empty() {
        items.push(
            PickerItem::action("Copy todos as Markdown", "/todo copy")
                .detail("Render the current list without changing it"),
        );
        items.push(
            PickerItem::action("Export todos to TODO.md", "/todo export")
                .detail("Write the current list to the workspace")
                .badge("DESTRUCTIVE"),
        );
    }
    for todo in &state.todos {
        let arg = quoted_arg(&todo.text);
        if todo.status != "done" {
            items.push(
                PickerItem::action(
                    format!("Complete: {}", todo.text),
                    format!("/todo done {}", arg),
                )
                .detail(format!("Current status: {}", todo.status))
                .badge("DONE"),
            );
        }
        if todo.status != "dropped" {
            items.push(
                PickerItem::action(
                    format!("Drop: {}", todo.text),
                    format!("/todo drop {}", arg),
                )
                .detail(format!("Current status: {}", todo.status))
                .badge("DESTRUCTIVE"),
            );
        }
        items.push(
            PickerItem::action(
                format!("Remove: {}", todo.text),
                format!("/todo rm {}", arg),
            )
            .detail(format!("Current status: {}", todo.status))
            .badge("DESTRUCTIVE"),
        );
    }
    PickerSpec::new("Todo workflow", items)
}

pub(crate) fn handle_todo(
    args: &str,
    state: &mut ModeState,
    context: &SlashContext<'_>,
) -> Result<String, String> {
    let argv = split_args(args);
    let verb = argv.first().map(String::as_str).unwrap_or("list");
    let text = argv
        .split_first()
        .map(|(_, rest)| rest.iter().cloned().collect::<Vec<_>>().join(" "))
        .unwrap_or_default();
    if verb.is_empty() || verb == "list" {
        if state.todos.is_empty() {
            return Ok("Todo list is empty.".into());
        }
        return Ok(state
            .todos
            .iter()
            .enumerate()
            .map(|(index, todo)| {
                format!(
                    "{}. [{}] {}",
                    index + usize::from(true),
                    todo.status,
                    todo.text
                )
            })
            .collect::<Vec<_>>()
            .join("\n"));
    }
    if verb == "add" || verb == "start" {
        if text.is_empty() {
            return Err(format!("Usage: /todo {} <task>", verb));
        }
        state.todos.push(TodoState {
            text: text.clone(),
            status: if verb == "start" {
                "in_progress".into()
            } else {
                "pending".into()
            },
            created_at: now_text(),
        });
        return Ok(format!("Todo added: {}", text));
    }
    if verb == "done" || verb == "drop" || verb == "rm" {
        let needle = text.to_ascii_lowercase();
        let Some(index) = state
            .todos
            .iter()
            .position(|todo| todo.text.to_ascii_lowercase().contains(&needle))
            .or_else(|| {
                text.parse::<usize>()
                    .ok()
                    .and_then(|n| n.checked_sub(usize::from(true)))
                    .filter(|&n| n < state.todos.len())
            })
        else {
            return Err(format!(
                "Todo not found: {}",
                if text.is_empty() { "(missing)" } else { &text }
            ));
        };
        let todo_text = state.todos[index].text.clone();
        if verb == "rm" {
            state.todos.remove(index);
        } else {
            state.todos[index].status = if verb == "done" {
                "done".into()
            } else {
                "dropped".into()
            };
        }
        return Ok(format!(
            "{} todo: {}",
            if verb == "rm" { "Removed" } else { "Updated" },
            todo_text
        ));
    }
    if verb == "copy" || verb == "export" {
        let md = if state.todos.is_empty() {
            "- [ ]".into()
        } else {
            state
                .todos
                .iter()
                .map(|todo| {
                    format!(
                        "- [{}] {}",
                        if todo.status == "done" { "x" } else { " " },
                        todo.text
                    )
                })
                .collect::<Vec<_>>()
                .join("\n")
        };
        if verb == "copy" {
            return Ok(md);
        }
        let target = resolve_cwd_path(context.cwd, if text.is_empty() { "TODO.md" } else { &text });
        if let Some(parent) = target.parent() {
            fs::create_dir_all(parent).map_err(|e| e.to_string())?;
        }
        fs::write(&target, format!("{}\n", md)).map_err(|e| e.to_string())?;
        return Ok(format!("Todos exported to {}", target.display()));
    }
    if verb == "import" {
        let target = resolve_cwd_path(context.cwd, if text.is_empty() { "TODO.md" } else { &text });
        let raw = fs::read_to_string(&target).map_err(|e| e.to_string())?;
        state.todos = raw
            .lines()
            .filter_map(|line| {
                let trimmed = line.trim_start();
                let after_open = trimmed.strip_prefix("- [")?;
                let status_mark = after_open.chars().next()?;
                let text = after_open
                    .get(status_mark.len_utf8()..)?
                    .strip_prefix("]")?
                    .trim()
                    .to_string();
                if text.is_empty() {
                    return None;
                }
                Some(TodoState {
                    text,
                    status: if status_mark == 'x' || status_mark == 'X' {
                        "done".into()
                    } else {
                        "pending".into()
                    },
                    created_at: now_text(),
                })
            })
            .collect();
        return Ok(format!(
            "Imported {} todos from {}",
            state.todos.len(),
            target.display()
        ));
    }
    Err("Usage: /todo [list|add|start|done|drop|rm|copy|export|import]".into())
}
