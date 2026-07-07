//! Slash help, builtin routing, model/settings slash, and self-update.

use serde_json::{json, Value};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::env;
use std::fs;

use crate::cli::auth::{format_auth_status, logout, start_login};
use crate::cli::commands::{discover_file_commands, DiscoveredCommand};
use crate::cli::config::load_config;
use crate::cli::config::schema::config_command;
use crate::{config_path, read_json, session_root, slash, Args};

const SLASH_COMMANDS: &[(&str, &str)] = &[
    ("settings", "Open settings menu"), ("setup", "Open provider setup"), ("plan", "Toggle plan mode"),
    ("plan-review", "Review latest plan"), ("goal", "Toggle goal mode"), ("loop", "Toggle loop mode"), ("model", "Switch model"),
    ("fast", "Toggle priority service tier"), ("advisor", "Toggle advisor reviewer"),
    ("export", "Export session"), ("dump", "Dump session"), ("share", "Share session"),
    ("collab", "Collaborate via relay"), ("join", "Join shared session"), ("leave", "Leave collab"),
    ("browser", "Configure browser runtime"), ("copy", "Copy conversation text"), ("todo", "Manage todos"),
    ("session", "Session management"), ("jobs", "Show jobs"), ("usage", "Show provider usage"),
    ("stats", "Launch stats dashboard"), ("changelog", "Show changelog"), ("hotkeys", "Show hotkeys"),
    ("approval", "Configure tool approval policy"),
    ("tools", "Show tools"), ("context", "Show context usage"), ("extensions", "Manage extensions"),
    ("agents", "Agent controls"), ("branch", "Create branch"), ("fork", "Create fork"), ("tree", "Navigate tree"),
    ("login", "Automated OAuth login"), ("logout", "Logout provider"), ("mcp", "Manage MCP servers"),
    ("ssh", "Manage SSH hosts"), ("new", "Start new session"), ("fresh", "Reset provider stream state"),
    ("drop", "Drop current session"), ("compact", "Compact session"), ("shake", "Shake session context"),
    ("handoff", "Hand off session"), ("resume", "Resume session"), ("btw", "Side question"),
    ("tan", "Background agent"), ("omfg", "Forge local rule"), ("retry", "Retry last failed turn"),
    ("debug", "Open debug tools"), ("memory", "Memory maintenance"), ("rename", "Rename session"),
    ("move", "Move session workspace"), ("marketplace", "Manage marketplace plugins"),
    ("plugins", "Manage installed plugins"), ("reload-plugins", "Reload plugins"), ("hooks", "Show lifecycle hooks"),
    ("update", "Run automated update"), ("force", "Force next tool"), ("exit", "Exit"), ("quit", "Quit"),
];

fn format_slash_help() -> String {
    let mut out = String::from("Jeden slash commands matching the OMP slash inventory:\n");
    for (name, description) in SLASH_COMMANDS {
        if *name == "update" { continue; }
        out.push_str(&format!("/{:<15} {}\n", name, description));
    }
    out.push_str("\nJeden-only conveniences:\n/help            show this command list\n/commands        show this command list\n/update          run automated git pull/build update\n");
    out
}

/// True for every slash command Jeden handles itself (canonical list + aliases).
/// Unknown slash input forwards to the model as a prompt, matching OMP, instead
/// of hard-erroring.
pub(crate) fn is_builtin_slash(command: &str) -> bool {
    let name = command.trim().trim_start_matches('/');
    const ALIASES: &[&str] = &[
        "help", "commands", "setup", "providers", "models", "switch",
        "stats", "debug", "status", "guided-goal", "clear",
    ];
    ALIASES.contains(&name) || SLASH_COMMANDS.iter().any(|(n, _)| *n == name)
}

fn update_tool(env_name: &str, default: &str) -> String {
    #[cfg(test)]
    {
        return env::var(env_name).ok().filter(|value| !value.trim().is_empty()).unwrap_or_else(|| default.into());
    }
    #[cfg(not(test))]
    {
        let _ = env_name;
        default.into()
    }
}

fn run_update_step(label: &str, program: &str, args: &[&str], cwd: &Path) -> Result<String, String> {
    let mut command = Command::new(program);
    command.args(args).current_dir(cwd).stdin(Stdio::null());
    if program.ends_with("git") {
        command.env("GIT_TERMINAL_PROMPT", "0").env("GCM_INTERACTIVE", "never");
    }
    let output = command
        .output()
        .map_err(|error| format!("{} failed to start {}: {}", label, program, error))?;
    let stdout = String::from_utf8_lossy(&output.stdout).trim().to_string();
    let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
    let detail = [stdout, stderr].into_iter().filter(|part| !part.is_empty()).collect::<Vec<_>>().join("\n");
    if output.status.success() {
        Ok(if detail.is_empty() { format!("{}: ok", label) } else { format!("{}:\n{}", label, detail) })
    } else {
        Err(if detail.is_empty() {
            format!("{} failed with {}", label, output.status)
        } else {
            format!("{} failed with {}\n{}", label, output.status, detail)
        })
    }
}

pub(crate) fn update_command() -> Result<String, String> {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let git = update_tool("JEDEN_UPDATE_GIT_BIN", "git");
    let cargo = update_tool("JEDEN_UPDATE_CARGO_BIN", "cargo");
    let before = run_update_step("git head before", &git, &["rev-parse", "--short", "HEAD"], &root)?;
    let pull = run_update_step("git pull --ff-only", &git, &["pull", "--ff-only"], &root)?;
    let build = run_update_step("cargo build --release", &cargo, &["build", "--release"], &root)?;
    let after = run_update_step("git head after", &git, &["rev-parse", "--short", "HEAD"], &root)?;
    Ok(format!("Jeden update completed\nSource: {}\n\n{}\n\n{}\n\n{}\n\n{}", root.display(), before, pull, build, after))
}

fn handle_model_slash(cwd: &Path, current_model: Option<&str>, args: &str) -> Result<String, String> {
    let next = args.trim();
    if next.is_empty() {
        let configured = current_model
            .map(str::to_string)
            .or_else(|| load_config(cwd).model)
            .or_else(|| env::var("JEDEN_MODEL").ok())
            .or_else(|| env::var("MODEL").ok())
            .unwrap_or_else(|| "default".into());
        return Ok(format!("Current model route: {}.", configured));
    }
    let path = config_path(cwd);
    let mut config = read_json::<Value>(&path);
    if !config.is_object() { config = json!({}); }
    config.as_object_mut().expect("object").insert("model".into(), Value::String(next.to_string()));
    if let Some(parent) = path.parent() { fs::create_dir_all(parent).map_err(|error| error.to_string())?; }
    fs::write(&path, serde_json::to_string_pretty(&config).map_err(|error| error.to_string())? + "\n").map_err(|error| error.to_string())?;
    Ok(format!("Model route set to {}.", next))
}

fn handle_settings_slash(cwd: &Path, args: &str) -> Result<String, String> {
    let trimmed = args.trim();
    if trimmed.is_empty() || trimmed == "status" {
        return Ok(format_auth_status(cwd));
    }
    let mut json = false;
    let positionals = trimmed
        .split_whitespace()
        .filter_map(|part| {
            if part == "--json" {
                json = true;
                None
            } else {
                Some(part.to_string())
            }
        })
        .collect::<Vec<_>>();
    let Some(verb) = positionals.first().map(String::as_str) else {
        return Ok(format_auth_status(cwd));
    };
    if !matches!(verb, "list" | "path" | "get" | "set" | "reset") {
        return Err("Usage: /settings [status|list|path|get <key>|set <key> <value>|reset <key>] [--json]".into());
    }
    // max_tokens/max_steps are irrelevant to config_command; Default fills them
    // number-free.
    config_command(&Args {
        command: "config".into(),
        cwd: cwd.to_path_buf(),
        json,
        positionals,
        ..Default::default()
    })
}

/// List discovered file-based custom commands for /help in provider-precedence order.
fn list_file_commands(cwd: &Path) -> Vec<DiscoveredCommand> {
    discover_file_commands(cwd)
}

pub(crate) fn handle_slash(cwd: &Path, input: &str, model: Option<&str>) -> Result<String, String> {
    let trimmed = input.trim();
    let mut parts = trimmed.split_whitespace();
    let command = parts.next().unwrap_or("");
    if command.eq_ignore_ascii_case("/update") {
        return update_command();
    }
    let session_root = session_root();
    let slash_context = slash::SlashContext { cwd, model, session_root: &session_root };
    if let Some(result) = slash::handle_local(&slash_context, trimmed) {
        return result;
    }
    match command {
        "/help" | "/commands" => {
            let mut help = format_slash_help();
            let file_cmds = list_file_commands(cwd);
            if !file_cmds.is_empty() {
                help.push_str("\nFile-based custom commands:\n");
                for command in file_cmds {
                    help.push_str(&format!("/{}\t{}\n", command.name, command.source));
                }
            }
            Ok(help)
        }
        "/settings" => handle_settings_slash(cwd, parts.collect::<Vec<_>>().join(" ").as_str()),
        "/setup" | "/providers" => Ok(format_auth_status(cwd)),
        "/login" => start_login(cwd, parts.collect::<Vec<_>>().join(" ").as_str()),
        "/logout" => logout(cwd, parts.collect::<Vec<_>>().join(" ").as_str()),
        "/model" | "/models" | "/switch" => handle_model_slash(cwd, model, parts.collect::<Vec<_>>().join(" ").as_str()),
        "/exit" | "/quit" => Ok("Exit is handled by the interactive input loop.".into()),
        _ => Err(format!("Unknown Rust slash command: {}", command)),
    }
}
