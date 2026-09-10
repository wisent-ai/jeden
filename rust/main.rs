use serde::Deserialize;
use serde_json::json;
use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

pub mod agent;
pub mod autonomy;
pub mod capability;
pub mod cas;
pub mod cli;
pub mod collab;
pub mod conformance;
pub mod completion;
pub mod context;
pub mod control_plane;
pub mod eval;
pub mod goal_lifecycle;
pub mod hooks;
pub mod marketplace;
pub mod mcp;
pub mod memory;
pub mod model_router;
pub mod onboarding;
pub mod probierz;
pub mod protocol;
pub mod qr;
pub mod report;
pub mod roadmap;
pub mod routing;
pub mod rpc;
pub mod sdk;
pub mod slash;
pub mod task_runtime;
pub mod telemetry;
pub mod tool_runtime;
pub mod tool_services;
pub mod tools;
pub mod tui;
pub mod update;

pub(crate) use cli::commands::expand::resolve_file_command;
pub(crate) use cli::completions::completions_command;
pub(crate) use cli::config::schema::config_command;
pub(crate) use cli::config::{load_config, Config};
pub(crate) use cli::invocation::{parse_args, usage, Args};

pub(crate) use cli::gallery::gallery_command;
pub(crate) use cli::run::interactive::interactive;
pub(crate) use cli::run::slash::{handle_slash, is_builtin_slash, update_command};
pub(crate) use cli::sessions::{
    artifact_command, export_session_command, list_artifacts_command, list_sessions,
    read_session_value, recall_conversation_command, recall_conversation_text,
    render_session_export, resume_command, search_sessions_command, session_conversation_turns,
};
pub(crate) use cli::stats::stats_command;
pub(crate) use cli::token::token_command;
pub(crate) use cli::workspace::command as workspace_command;
pub(crate) use cli::worktree::worktree_command;

pub(crate) const JEDEN_VERSION: &str = env!("JEDEN_VERSION");

fn version_text() -> String {
    format!("jeden {JEDEN_VERSION}")
}


fn read_json<T: for<'a> Deserialize<'a> + Default>(path: &Path) -> T {
    fs::read_to_string(path)
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_default()
}

fn parse_env_value(raw: &str) -> String {
    let mut value = raw.trim().to_string();
    if let Some(index) = value.find(" #") {
        value.truncate(index);
        value = value.trim().to_string();
    }
    let unquoted = value
        .strip_prefix('"')
        .and_then(|v| v.strip_suffix('"'))
        .or_else(|| value.strip_prefix('\'').and_then(|v| v.strip_suffix('\'')));
    if let Some(inner) = unquoted {
        value = inner.to_string();
    }
    value.replace("\\n", "\n")
}

fn load_env_path(path: &Path, loaded: &mut Vec<String>) -> Result<(), String> {
    let text = match fs::read_to_string(path) {
        Ok(text) => text,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(error.to_string()),
    };
    for line in text.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() || trimmed.starts_with('#') {
            continue;
        }
        let Some((key, raw_value)) = trimmed.split_once('=') else {
            continue;
        };
        let key = key.trim();
        if key.is_empty() || env::var_os(key).is_some() {
            continue;
        }
        env::set_var(key, parse_env_value(raw_value));
        loaded.push(key.to_string());
    }
    Ok(())
}

fn load_env_files(cwd: &Path) -> Result<Vec<String>, String> {
    let mut loaded = Vec::new();
    for name in [".env", ".env.local", ".env.production", ".env.vercel"] {
        load_env_path(&cwd.join(name), &mut loaded)?;
    }
    load_env_path(&dirs_home().join(".jeden/.env"), &mut loaded)?;
    loaded.sort();
    loaded.dedup();
    Ok(loaded)
}

fn config_path(cwd: &Path) -> PathBuf {
    cwd.join(".jeden/config.json")
}
fn session_root() -> PathBuf {
    env::var_os("JEDEN_SESSION_ROOT")
        .map(PathBuf::from)
        .unwrap_or_else(|| dirs_home().join(".jeden/sessions"))
}
fn dirs_home() -> PathBuf {
    env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("."))
}
fn legacy_user_config_path() -> PathBuf {
    dirs_home().join(".jeden/config.json")
}
fn user_config_path() -> PathBuf {
    dirs_home().join(".jeden/config.yml")
}

pub fn main() -> ExitCode {
    let argv = env::args().skip(usize::from(true)).collect::<Vec<_>>();
    let mut args = match parse_args(argv) {
        Ok(v) => v,
        Err(e) => {
            eprintln!("Error: {}\n{}", e, usage());
            return ExitCode::FAILURE;
        }
    };
    if args.command == "version" {
        println!("{}", version_text());
        return ExitCode::SUCCESS;
    }
    if matches!(
        args.command.as_str(),
        "interactive" | "run" | "pursue" | "rpc" | "acp"
    ) {
        match cli::workspace::effective_cwd(&args.cwd, args.cwd_explicit) {
            Ok(cwd) => args.cwd = cwd,
            Err(error) => {
                eprintln!("Error: {error}");
                return ExitCode::FAILURE;
            }
        }
    }
    if let Err(error) = load_env_files(&args.cwd) {
        eprintln!("Error: failed to load environment files: {}", error);
        return ExitCode::FAILURE;
    }
    tui::theme::init(&args.cwd);
    if args.command == "doctor" {
        let report = conformance::health::doctor(&args.cwd);
        match serde_json::to_string(&report) {
            Ok(text) => println!("{text}"),
            Err(error) => {
                eprintln!("Error: failed to serialize doctor report: {error}");
                return ExitCode::FAILURE;
            }
        }
        return if report.healthy {
            ExitCode::SUCCESS
        } else {
            ExitCode::FAILURE
        };
    }
    if args.command == "conformance" {
        return match conformance::run(&args.cwd)
            .and_then(|report| conformance::canonical_json(&report).map(|text| (report, text)))
        {
            Ok((report, text)) => {
                print!("{text}");
                if report.complete {
                    ExitCode::SUCCESS
                } else {
                    ExitCode::FAILURE
                }
            }
            Err(error) => {
                eprintln!("Error: {error}");
                ExitCode::FAILURE
            }
        };
    }
    let result = match args.command.as_str() {
        "help" => Ok(usage()),
        "interactive" => interactive(&args),
        "run" => agent::run_command(&args),
        "pursue" => autonomy::command(&args),
        "todo" => completion::command(&args),
        "rpc" => rpc::serve_stdio().map(|_| String::new()),
        "headless" => rpc::serve_headless_cli(&args.positionals, &args.cwd.join(".jeden/headless"))
            .map(|_| String::new()),
        "acp" => rpc::serve_acp_stdio().map(|_| String::new()),
        "collab-relay" => {
            let addr = args
                .positionals
                .first()
                .cloned()
                .unwrap_or_else(|| "127.0.0.1:8877".to_string());
            collab::serve(&addr).map(|_| String::new())
        }
        "sessions" => Ok(list_sessions(
            args.positionals.first().and_then(|s| s.parse().ok()),
        )),
        "show" => args
            .positionals
            .first()
            .map(|id| {
                render_session_export(
                    &read_session_value(id).unwrap_or_else(|e| json!({"error": e})),
                    "json",
                )
                .unwrap_or_default()
            })
            .ok_or("show requires a session id".into()),
        "export" => export_session_command(&args),
        "artifacts" => args
            .positionals
            .first()
            .map(|id| list_artifacts_command(id))
            .unwrap_or_else(|| Err("artifacts requires a session id".into())),
        "artifact" => artifact_command(&args),
        "tools" => Ok(tools::tools_output(&args.cwd, args.json)),
        "search-sessions" => search_sessions_command(&args),
        "resume" => resume_command(&args),
        "recall_conversation" | "recall-conversation" => recall_conversation_command(&args),
        "update" => update_command(),
        "config" => config_command(&args),
        "workspace" => workspace_command(&args),
        "contracts" => cli::contracts::command(&args),
        "roadmap" => roadmap::execute(&args.cwd, &args.positionals, args.json)
            .map_err(|error| error.to_string()),
        "probierz" => probierz::command(&args),
        "capabilities" => {
            if args.json {
                capability::status_json(&args.cwd).map(|json| json + "\n")
            } else {
                Ok(capability::status_text(&args.cwd) + "\n")
            }
        }
        "completions" => completions_command(&args),
        "worktree" => worktree_command(&args),
        "token" => token_command(&args),
        "stats" => stats_command(&args),
        "gallery" => gallery_command(&args),
        other => Err(format!("unknown command: {}", other)),
    };
    match result {
        Ok(text) => {
            print!("{}", text);
            ExitCode::SUCCESS
        }
        Err(e) => {
            eprintln!("Error: {}", e);
            ExitCode::FAILURE
        }
    }
}
