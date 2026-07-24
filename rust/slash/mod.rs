use std::path::Path;

use crate::slash::common::{split_args, split_head};
pub(crate) use crate::slash::state::read_mode_state;
use crate::slash::state::write_mode_state;
use crate::tools;
use crate::tui::{PickerItem, PickerSpec};

mod browser;
mod commands;
mod common;
mod modes;
mod plugins;
mod session;
mod state;
mod validate;

pub(crate) use plugins::ops::{installed_plugin_command_dirs, installed_plugin_hook_configs};

#[derive(Debug, Clone)]
pub struct SlashContext<'a> {
    pub cwd: &'a Path,
    pub model: Option<&'a str>,
    pub session_root: &'a Path,
}

pub fn handle_local(context: &SlashContext<'_>, input: &str) -> Option<Result<String, String>> {
    let trimmed = input.trim();
    let (command, args) = split_head(trimmed);
    let command = command.to_ascii_lowercase();
    let mut state = read_mode_state(context.cwd);
    let mut changed = false;
    let result = match command.as_str() {
        "/plan" => { changed = args.trim() != "status"; Some(modes::handle_plan(args, &mut state)) },
        "/plan-review" => Some(modes::handle_plan_review(&state)),
        "/goal" => { changed = !matches!(split_head(args).0, "" | "show" | "status"); Some(modes::handle_goal(args, &mut state)) },
        "/guided-goal" => { changed = true; Some(modes::handle_guided_goal(args, &mut state)) },
        "/loop" => { changed = split_head(args).0 != "status"; Some(modes::handle_loop(args, &mut state)) },
        "/fast" => { changed = split_head(args).0 != "status"; Some(modes::handle_fast(args, &mut state)) },
        "/advisor" => { changed = !matches!(split_head(args).0, "" | "status" | "dump"); Some(modes::handle_advisor(args, &mut state, context)) },
        "/approval" => { changed = !matches!(split_head(args).0, "" | "status"); Some(modes::handle_approval(args, &mut state)) },
        "/tools" => Some(Ok(if split_args(args).iter().any(|arg| arg == "--json") { tools::tools_json(context.cwd) } else { tools::tools_slash_text(context.cwd) })),
        "/stats" | "/debug" => Some(commands::handle_doctor(context)),
        "/usage" => Some(commands::usage::handle_usage(args, context)),

        "/session" | "/sessions" => Some(modes::session::handle_session(args, context)),
        "/roles" | "/role" => Some(modes::session::handle_roles(context)),
        "/prompt" => Some(crate::agent::system_prompt_checked(context.cwd)),
        "/todo" => { changed = !matches!(split_head(args).0, "" | "list" | "copy" | "export"); Some(modes::todo::handle_todo(args, &mut state, context)) },
        "/roadmap" => Some(
            crate::roadmap::split_command_line(args)
                .map_err(|error| error.to_string())
                .and_then(|mut command_args| {
                    let json = command_args.iter().any(|argument| argument == "--json");
                    command_args.retain(|argument| argument != "--json");
                    crate::roadmap::execute(context.cwd, &command_args, json)
                        .map_err(|error| error.to_string())
                }),
        ),
        "/mcp" => Some(commands::mcp::handle_mcp(args, context)),
        "/ssh" => Some(commands::ssh::handle_ssh(args, context)),
        "/browser" => Some(browser::handle_browser(args, context)),
        "/extensions" => Some(plugins::handle_extensions(context)),
        "/status" => Some(Ok(crate::capability::status_text(context.cwd))),
        "/plugins" => Some(plugins::handle_plugins(args, context)),
        "/hooks" => Some(Ok(crate::hooks::describe_hooks(context.cwd))),
        "/reload-plugins" => Some(plugins::handle_reload_plugins(context)),
        "/marketplace" => Some(plugins::marketplace::handle_marketplace(args, context)),
        "/copy" => Some(session::clipboard::handle_copy(args, context)),
        "/collab" => Some(session::collab::handle_collab(args, context)),
        "/join" => Some(session::collab::handle_join(args, context)),
        "/leave" => Some(session::collab::handle_leave(context)),
        "/dump" => Some(session::handle_dump(args, context)),
        "/export" => Some(session::handle_export(args, context)),
        "/share" => Some(session::handle_share(args, context)),
        "/omfg" => Some(session::handle_omfg(args, context)),
        "/force" | "/force:" => { changed = true; Some(modes::handle_force(args, &mut state, context)) },
        "/checkpoint" | "/rewind" => Some(Err(format!(
            "{} must be executed through the agent runner so it can mutate the live durable conversation.",
            command
        ))),
        "/rebuild" | "/self-rebuild" => Some(Err(
            "/rebuild is available only in an interactive session so it can preserve the live conversation."
                .into(),
        )),
        "/retry" => Some(Err("/retry must be executed through the agent runner so it can replay lastFailedTask.".into())),
        "/btw" => Some(Err("/btw must be executed through the agent runner so it can run the side question.".into())),
        "/memory" => Some(commands::memory::handle_memory(args, context)),
        "/branch" | "/fork" | "/tree" => Some(modes::handle_branching(command.as_str(), args, &state)),
        "/new" | "/fresh" | "/drop" | "/shake" | "/resume" | "/rename" | "/move" => {
            changed = command == "/shake";
            modes::session::handle_lifecycle(command.as_str(), args, &mut state, context)
        },
        "/agents" => Some(commands::agents::handle_agents(args, context)),
        "/jobs" => Some(session::handle_jobs(context)),
        "/changelog" => Some(commands::handle_changelog()),
        "/hotkeys" => Some(Ok("Jeden input:\nType a prompt on the `jeden >` line and press Enter.\nSlash commands such as /help and /update run from the same line.\nCtrl-C exits.".into())),
        "/tan" => Some(session::handle_tan(args, context)),
        _ => None,
    };
    if changed {
        if let Some(Ok(_)) = &result {
            if let Err(error) = write_mode_state(context.cwd, &state) {
                return Some(Err(error));
            }
        }
    }
    result
}

pub(crate) fn activate_roadmap_work(
    cwd: &Path,
    item_id: &str,
    objective: &str,
    plan: &str,
    todos: &[(String, String)],
) -> Result<(), String> {
    state::mutate_mode_state(cwd, |state| {
        state.goal.enabled = true;
        state.goal.paused = false;
        state.goal.objective = objective.to_string();
        state.plan.enabled = true;
        state.plan.latest_plan = plan.to_string();
        state.todos = todos
            .iter()
            .map(|(text, status)| state::TodoState {
                text: text.clone(),
                status: status.clone(),
                created_at: crate::agent::now_stamp(),
            })
            .collect();
        state.active_roadmap_item = Some(item_id.to_string());
        Ok(())
    })
}

pub(crate) fn update_session_pointer(cwd: &Path, path: &Path) -> Result<(), String> {
    state::mutate_mode_state(cwd, |state| {
        state.last_session_path = Some(path.to_path_buf());
        Ok(())
    })
}
fn read_only_picker(title: &str, text: String) -> PickerSpec {
    let mut items = text
        .lines()
        .filter(|line| !line.trim().is_empty())
        .map(|line| PickerItem::action(line, "").disabled(true))
        .collect::<Vec<_>>();
    if items.is_empty() {
        items.push(PickerItem::action("No information available", "").disabled(true));
    }
    PickerSpec::new(title, items)
}

fn tools_picker(context: &SlashContext<'_>) -> PickerSpec {
    let mut items = tools::list_tools(context.cwd)
        .into_iter()
        .map(|tool| {
            PickerItem::action(tool.name, "")
                .detail(tool.description)
                .badge("TOOL")
                .disabled(true)
        })
        .collect::<Vec<_>>();
    if items.is_empty() {
        items.push(PickerItem::action("No tools available", "").disabled(true));
    }
    PickerSpec::new("Available tools", items)
}

fn capability_picker(context: &SlashContext<'_>) -> PickerSpec {
    let mut items = crate::capability::management_items(context.cwd)
        .into_iter()
        .map(|(label, detail, badge, action, disabled)| {
            let mut item = PickerItem::action(label, action.unwrap_or_default())
                .detail(detail)
                .badge(badge);
            if disabled {
                item = item.disabled(true);
            }
            item
        })
        .collect::<Vec<_>>();
    if items.is_empty() {
        items.push(PickerItem::action("No capabilities discovered", "").disabled(true));
    }
    PickerSpec::new("Capability status", items)
}

pub(crate) fn interactive_picker(
    context: &SlashContext<'_>,
    input: &str,
) -> Option<Result<PickerSpec, String>> {
    let (command, args) = split_head(input.trim());
    if !args.trim().is_empty() {
        return None;
    }
    let lang = crate::cli::i18n::lang_code(context.cwd);
    let view = crate::capability::view_descriptor(context.cwd, command)?;
    if !view.health.is_executable() || !view.ui.executable {
        let detail = view
            .health
            .detail
            .unwrap_or_else(|| "Capability backend unavailable".into());
        return Some(Ok(PickerSpec::new(
            format!("{} unavailable", view.ui.label),
            vec![PickerItem::action(view.ui.label, "")
                .detail(detail)
                .badge(crate::cli::i18n::tr(&lang, "badge.unavailable"))
                .disabled(true)],
        )));
    }
    let state = read_mode_state(context.cwd);
    let picker = match command.to_ascii_lowercase().as_str() {
        "/plan" => Ok(modes::todo::plan_picker(&state, &lang)),
        "/guided-goal" | "/goal" => Ok(modes::todo::goal_picker(&state, &lang)),
        "/loop" => Ok(modes::todo::loop_picker(&state)),
        "/fast" => Ok(modes::todo::fast_picker(&state)),
        "/advisor" => Ok(modes::session::advisor_picker(&state, context)),
        "/approval" => Ok(modes::session::approval_picker(&state, &lang)),
        "/todo" => Ok(modes::todo::todo_picker(&state, &lang)),
        "/roadmap" => crate::roadmap::picker(context.cwd).map_err(|error| error.to_string()),
        "/session" | "/sessions" => Ok(modes::session::session_picker(context)),
        "/roles" | "/role" => Ok(modes::session::roles_picker(&state, context)),
        "/tree" | "/branch" | "/fork" => Ok(modes::session::tree_picker(&state, &lang)),
        "/new" | "/fresh" | "/drop" | "/shake" | "/resume" | "/rename" | "/move" => {
            Ok(modes::session::lifecycle_picker(&state, context))
        }
        "/mcp" => Ok(commands::mcp::mcp_picker(context)),
        "/ssh" => Ok(commands::ssh::ssh_picker(context)),
        "/memory" => commands::memory::memory_picker(),
        "/usage" => Ok(commands::usage::usage_picker(context)),
        "/browser" => Ok(browser::browser_picker(context)),
        "/stats" => Ok(commands::stats_picker(context)),
        "/debug" => Ok(commands::debug_picker(context)),
        "/tools" => Ok(tools_picker(context)),
        "/extensions" => Ok(plugins::extensions_picker(context)),
        "/status" => Ok(capability_picker(context)),
        "/plugins" => Ok(plugins::plugins_picker(context)),
        "/reload-plugins" => Ok(plugins::reload_plugins_picker(context)),
        "/marketplace" => Ok(plugins::marketplace::marketplace_picker(context)),
        "/jobs" => Ok(session::jobs_picker(context)),
        "/collab" => Ok(session::collab_picker(context)),
        "/join" => Ok(session::join_picker(context)),
        "/leave" => Ok(session::leave_picker(context)),
        "/share" => Ok(session::share_picker(context)),
        "/export" => Ok(session::export_picker(context)),
        "/dump" => Ok(session::dump_picker(context)),
        "/copy" => Ok(session::copy_picker()),
        "/tan" => Ok(session::tan_picker(context)),
        "/omfg" => Ok(session::omfg_picker(context)),
        "/agents" => Ok(commands::agents::agents_picker(context)),
        "/hooks" => Ok(read_only_picker("Lifecycle hooks", crate::hooks::describe_hooks(context.cwd))),
        "/changelog" => commands::handle_changelog()
            .map(|text| read_only_picker("Changelog", text)),
        "/hotkeys" => Ok(read_only_picker("Keyboard shortcuts", "Enter submit\nAlt-Enter newline\nUp/Down navigate\nTab complete\nEsc close or cancel\nCtrl-C exit".into())),
        _ => return None,
    };
    Some(picker)
}
