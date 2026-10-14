use serde_json::json;
use std::env;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use slash::common::dirs_home;

pub mod agent;
pub mod autonomy;
pub mod capability;
pub mod cas;
pub mod chat_memory;
pub mod cli;
pub mod clock;
pub mod collab;
pub mod completion;
pub mod conformance;
pub mod context;
pub mod control_plane;
pub mod eval;
pub(crate) mod fleet;
pub mod goal_lifecycle;
pub mod hooks;
pub mod marketplace;
pub mod mcp;
pub mod memory;
pub mod model_router;
pub mod net;
pub mod onboarding;

// Moved to the modules that own them, under the paths every caller uses:
// the action protocol the agent speaks and the terminal QR rendering.
pub use agent::protocol;
pub use tui::qr;
pub mod report;
pub mod roadmap;
pub mod routing;
pub mod rpc;
pub mod sdk;
mod session_import;
pub mod slash;
pub mod task_runtime;
pub mod tool_runtime;
pub mod tool_services;
pub mod tools;
pub mod tui;
pub mod update;

pub(crate) use cli::commands::expand::resolve_file_command;
pub(crate) use cli::completions::completions_command;
pub(crate) use cli::config::schema::config_command;
pub(crate) use cli::config::{load_config, Config};
pub(crate) use cli::invocation::{parse_args, usage, Args, Command};

pub(crate) use cli::gallery::gallery_command;
pub(crate) use cli::run::interactive::interactive;
pub(crate) use cli::run::slash::{handle_slash, is_builtin_slash, update_command};
pub(crate) use cli::sessions::{
    artifact_command, export_session_command, list_artifacts_command, list_sessions,
    read_session_value, recall_conversation_text, render_session_export, resume_command,
    search_sessions_command, session_conversation_turns,
};
pub(crate) use cli::stats::stats_command;
pub(crate) use cli::token::token_command;
pub(crate) use cli::workspace::command as workspace_command;

pub(crate) const JEDEN_VERSION: &str = env!("JEDEN_VERSION");

fn version_text() -> String {
    format!("jeden {JEDEN_VERSION}")
}

fn config_path(cwd: &Path) -> PathBuf {
    cwd.join(".jeden/config.json")
}
fn session_root() -> PathBuf {
    env::var_os("JEDEN_SESSION_ROOT")
        .map(PathBuf::from)
        .unwrap_or_else(|| dirs_home().join(".jeden/sessions"))
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
            // A command line that does not parse is a usage error: exit 2,
            // every other failure exits 1.
            const USAGE_STATUS: u8 = 2;
            eprintln!("Error: {}\n{}", e, usage());
            return ExitCode::from(USAGE_STATUS);
        }
    };
    if args.command == Command::Version {
        println!("{}", version_text());
        return ExitCode::SUCCESS;
    }
    if args.command.workspace_bound() {
        match cli::workspace::effective_cwd(&args.cwd, args.cwd_explicit) {
            Ok(cwd) => args.cwd = cwd,
            Err(error) => {
                eprintln!("Error: {error}");
                return ExitCode::FAILURE;
            }
        }
    }
    match cli::config::env_files::load_env_files(&args.cwd) {
        Ok(loaded) => agent::credential::record_file_supplied(loaded),
        Err(error) => {
            eprintln!("Error: failed to load environment files: {}", error);
            return ExitCode::FAILURE;
        }
    }
    if let Err(error) = cli::config::check_config(&args.cwd)
        .and_then(|()| slash::state::check_mode_state(&args.cwd))
    {
        eprintln!("Error: {error}");
        return ExitCode::FAILURE;
    }
    tui::theme::init(&args.cwd);
    if args.command == Command::Doctor {
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
    if args.command == Command::Conformance {
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
    let result = match args.command {
        Command::Version | Command::Doctor | Command::Conformance => {
            unreachable!("{} answers before the workspace is read", args.command.name())
        }
        Command::Help => Ok(cli::invocation::command_usage(
            args.positionals.first().map(String::as_str),
        )),
        Command::Interactive => interactive(&args),
        Command::Run => agent::run_command(&args),
        Command::Pursue => autonomy::command(&args),
        Command::Todo => completion::command(&args),
        Command::Asks => completion::asks_command(&args),
        Command::Rpc => rpc::serve_stdio().map(|_| String::new()),
        Command::Headless => {
            rpc::serve_headless_cli(&args.positionals, &args.cwd.join(".jeden/headless"))
                .map(|_| String::new())
        }
        Command::Acp => rpc::serve_acp_stdio().map(|_| String::new()),
        Command::Relay => match args.positionals.as_slice() {
            [verb, addr] if verb == "serve" => collab::serve(addr).map(|_| String::new()),
            _ => Err(cli::invocation::refusal::usage(
                "relay serve takes the address to listen on: jeden relay serve <host:port>",
            )),
        },
        Command::Import => session_import::command(&args),
        Command::Sessions => match args.positionals.split_first() {
            None => list_sessions(None, args.json),
            Some((verb, query)) if verb == "search" => search_sessions_command(query, args.json),
            Some((raw, _)) => raw
                .parse()
                .map_err(|_| {
                    cli::invocation::refusal::usage(format!(
                        "sessions takes a whole number of sessions to list, or search QUERY, not {raw:?}"
                    ))
                })
                .and_then(|limit| list_sessions(Some(limit), args.json)),
        },
        Command::Copy => cli::clipboard::copy_command(&args),
        // A session that cannot be read or rendered is a failure with its
        // cause, never an exit 0 carrying an error object or nothing. Text is
        // the session's events one heading each; --json is the whole record.
        Command::Show => match args.positionals.first() {
            Some(id) => read_session_value(id)
                .map_err(|error| format!("session {id} cannot be read: {error}"))
                .and_then(|session| {
                    render_session_export(&session, if args.json { "json" } else { "markdown" })
                }),
            None => Err(cli::invocation::refusal::usage(
                "show requires a session id",
            )),
        },
        Command::Export => export_session_command(&args),
        Command::Artifacts => match args.positionals.first() {
            Some(id) => list_artifacts_command(id, args.json),
            None => Err(cli::invocation::refusal::usage(
                "artifacts requires a session id",
            )),
        },
        Command::Artifact => artifact_command(&args),
        Command::Tools => Ok(tools::tools_output(&args.cwd, args.json)),
        Command::Resume => resume_command(&args),
        Command::Update => update_command(),
        Command::Config => config_command(&args),
        Command::Workspace => workspace_command(&args),
        Command::Contracts => cli::contracts::command(&args),
        Command::Context => cli::context::command(&args),
        Command::Roadmap => roadmap::execute(&args.cwd, &args.positionals, args.json)
            .map_err(|error| error.to_string()),
        Command::Capabilities => {
            if args.json {
                capability::status_json(&args.cwd).map(|json| json + "\n")
            } else {
                Ok(capability::status_text(&args.cwd) + "\n")
            }
        }
        Command::Completions => completions_command(&args),
        Command::Token => token_command(&args),
        Command::Lease => cli::lease::command(&args),
        Command::ChatMemory => cli::chat_memory::command(&args),
        Command::Stats => stats_command(&args),
        Command::Gallery => gallery_command(&args),
    };
    match result {
        Ok(text) => {
            print!("{}", text);
            ExitCode::SUCCESS
        }
        Err(e) => {
            eprintln!("Error: {}", e);
            // A command's refusal of its own invocation exits 2, every other
            // failure 1 (cli.md rule 10).
            if cli::invocation::refusal::is_usage(&e) {
                ExitCode::from(2)
            } else {
                ExitCode::FAILURE
            }
        }
    }
}
