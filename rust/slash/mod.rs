use std::path::Path;

use crate::slash::common::{split_args, split_head};
use crate::slash::state::{read_mode_state, write_mode_state};
use crate::tools;

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

        "/session" => Some(modes::session::handle_session(args, context)),
        "/todo" => { changed = !matches!(split_head(args).0, "" | "list" | "copy" | "export"); Some(modes::todo::handle_todo(args, &mut state, context)) },
        "/mcp" => Some(commands::mcp::handle_mcp(args, context)),
        "/ssh" => Some(commands::ssh::handle_ssh(args, context)),
        "/browser" => Some(browser::handle_browser(args, context)),
        "/extensions" | "/status" => Some(plugins::handle_extensions(context)),
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
        "/retry" => Some(Err("/retry must be executed through the agent runner so it can replay lastFailedTask.".into())),
        "/btw" => Some(Err("/btw must be executed through the agent runner so it can run the side question.".into())),
        "/memory" => Some(commands::memory::handle_memory(args, context)),
        "/branch" | "/fork" | "/tree" => Some(modes::handle_branching(command.as_str(), args, &state)),
        "/new" | "/fresh" | "/drop" | "/shake" | "/resume" | "/rename" | "/move" => {
            changed = command == "/shake";
            modes::session::handle_lifecycle(command.as_str(), args, &mut state, context)
        },
        "/agents" => Some(Ok("Agent controls:\n- /tan <work> starts a detached local agent job tracked in session artifacts.\n- /advisor manages second-pass reviewer mode.\n- /jobs shows locally tracked background jobs.".into())),
        "/jobs" => Some(session::handle_jobs(context)),
        "/changelog" => Some(commands::handle_changelog()),
        "/hotkeys" => Some(Ok("Jeden input:\nType a prompt on the `jeden >` line and press Enter.\nSlash commands such as /help and /update run from the same line.\nCtrl-C exits.".into())),
        "/tan" => Some(session::handle_tan(args, context)),
        _ => None,
    };
    if changed {
        if let Some(Ok(_)) = &result {
            if let Err(error) = write_mode_state(context.cwd, &state) { return Some(Err(error)); }
        }
    }
    result
}
