use serde::Deserialize;
use serde_json::json;
use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

mod tui;
mod agent;
mod collab;
mod hooks;
mod mcp;
mod model_router;
mod protocol;
mod slash;
mod tools;
mod tool_runtime;
mod cli;

pub(crate) use cli::config::{load_config, Config};
pub(crate) use cli::config::schema::config_command;
pub(crate) use cli::commands::expand::resolve_file_command;
pub(crate) use cli::run::slash::{handle_slash, is_builtin_slash, update_command};
pub(crate) use cli::run::interactive::interactive;
pub(crate) use cli::sessions::{artifact_command, export_session_command, list_artifacts_command, list_sessions, read_session_value, recall_conversation_command, render_session_export, resume_command, search_sessions_command, session_conversation_turns};

#[derive(Debug, Clone, Default)]
pub(crate) struct Args {
    pub(crate) command: String,
    pub(crate) cwd: PathBuf,
    pub(crate) model: Option<String>,
    pub(crate) max_tokens: Option<u32>,
    pub(crate) max_steps: Option<u32>,
    pub(crate) allow_write: bool,
    pub(crate) allow_command: bool,
    pub(crate) yolo: bool,
    pub(crate) json: bool,
    pub(crate) positionals: Vec<String>,
}

fn usage() -> String {
    "Usage:\n  jeden [--cwd path] [--model name] [--max-tokens n] [--allow-write] [--allow-command] [--yolo|--auto-approve] [--max-steps n]\n  jeden run \"task\" [--json] [--cwd path] [--model name] [--max-tokens n] [--allow-write] [--allow-command] [--yolo|--auto-approve] [--max-steps n]\n  jeden sessions [limit]\n  jeden show <session-id-or-path>\n  jeden export <session-id-or-path> [output.json]\n  jeden artifacts <session-id-or-path>\n  jeden artifact <session-id-or-path> <name> [output]\n  jeden config [list|path|get <key>|set <key> <value>|reset <key>] [--json] [--cwd path]\n  jeden doctor [--cwd path]\n  jeden capabilities [--cwd path]\n\nSlash commands:\n  /login [provider]      use OMP auth-broker login/provider selection\n  /logout [provider]     use OMP auth-broker logout/provider selection\n  /settings              show auth and provider status\n  /model [name]          show or set model route\n  /mcp [list|tools|resources|prompts|test|reload|reconnect]\n  /marketplace [list|discover|installed|add|remove|install|uninstall|upgrade]\n  /plugins [list|enable|disable]\n  /approval [status|mode|set|reset]\n  /tools                 show available tools\n  /usage [show|reset]    show token/cost accounting\n  /browser [status|headless|visible]\n  /analytics             show local analytics\n  /plan [on|off|status]  control plan mode\n  /goal [set|done|drop]  control goal mode\n  /loop [on|off|status]  control continuation loop\n  /todo [list|add|done]  manage todos\n  /memory [stats|view|enqueue|rebuild|clear]\n  /copy <text>           copy text to clipboard if pbcopy exists\n  /collab [status|start|share|sync|stop]\n  /join <relay-file-url-or-path>\n  /leave\n  /dump [session]\n  /export [session] [--format json|text] [--output file]\n  /share [copy]\n  /omfg <complaint>\n  /tan <work>\n  /jobs\n  /changelog\n  /extensions\n  /reload-plugins\n  /retry\n  /btw <question>\n  /compact [focus]\n  /handoff [focus]\n  /clear|/new|/fresh\n  /fork | /branch <title> | /tree | /resume <session>\n  /rename <name> | /drop | /context | /move <dir>\n".to_string()
}

fn parse_args(argv: Vec<String>) -> Result<Args, String> {
    let mut rest = argv.into_iter();
    let first = rest.next();
    let mut command = first.unwrap_or_else(|| "interactive".to_string());
    if command == "--help" || command == "-h" {
        return Ok(Args { command: "help".into(), cwd: env::current_dir().unwrap_or_default(), ..Default::default() });
    }
    if matches!(command.as_str(), "resume" | "recall_conversation" | "recall-conversation" | "search-sessions") {
        return Ok(Args { command, cwd: env::current_dir().map_err(|e| e.to_string())?, positionals: rest.collect(), ..Default::default() });
    }
    if command.starts_with("--") { rest = std::iter::once(command).chain(rest).collect::<Vec<_>>().into_iter(); command = "interactive".into(); }
    let mut args = Args { command, cwd: env::current_dir().map_err(|e| e.to_string())?, ..Default::default() };
    while let Some(arg) = rest.next() {
        match arg.as_str() {
            "--cwd" => args.cwd = PathBuf::from(rest.next().ok_or("--cwd requires a value")?),
            "--model" => args.model = Some(rest.next().ok_or("--model requires a value")?),
            "--max-tokens" => args.max_tokens = Some(rest.next().ok_or("--max-tokens requires a value")?.parse().map_err(|_| "--max-tokens must be an integer")?),
            "--max-steps" => args.max_steps = Some(rest.next().ok_or("--max-steps requires a value")?.parse().map_err(|_| "--max-steps must be an integer")?),
            "--allow-write" => args.allow_write = true,
            "--allow-command" => args.allow_command = true,
            "--yolo" | "--auto-approve" => { args.yolo = true; args.allow_write = true; args.allow_command = true; },
            "--json" => args.json = true,
            other if other.starts_with("--") && (matches!(args.command.as_str(), "export") || (args.command == "run" && !args.positionals.is_empty())) => args.positionals.push(other.to_string()),
            other if other.starts_with("--") => return Err(format!("unknown option: {}", other)),
            other => args.positionals.push(other.to_string()),
        }
    }
    if args.command == "run" && args.positionals.is_empty() { return Err("run requires a task".into()); }
    if args.command == "interactive" && !args.positionals.is_empty() { return Err(format!("unknown command: {}", args.positionals.first().map(String::as_str).unwrap_or_default())); }
    Ok(args)
}

fn read_json<T: for<'a> Deserialize<'a> + Default>(path: &Path) -> T {
    fs::read_to_string(path).ok().and_then(|s| serde_json::from_str(&s).ok()).unwrap_or_default()
}

fn parse_env_value(raw: &str) -> String {
    let mut value = raw.trim().to_string();
    if let Some(index) = value.find(" #") {
        value.truncate(index);
        value = value.trim().to_string();
    }
    let unquoted = value.strip_prefix('"').and_then(|v| v.strip_suffix('"'))
        .or_else(|| value.strip_prefix('\'').and_then(|v| v.strip_suffix('\'')));
    if let Some(inner) = unquoted { value = inner.to_string(); }
    value.replace("\\n", "\n")
}

fn load_env_files(cwd: &Path) -> Result<Vec<String>, String> {
    let mut loaded = Vec::new();
    for name in [".env", ".env.local", ".env.production", ".env.vercel"] {
        let path = cwd.join(name);
        let text = match fs::read_to_string(&path) {
            Ok(text) => text,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
            Err(error) => return Err(error.to_string()),
        };
        for line in text.lines() {
            let trimmed = line.trim();
            if trimmed.is_empty() || trimmed.starts_with('#') { continue; }
            let Some((key, raw_value)) = trimmed.split_once('=') else { continue; };
            let key = key.trim();
            if key.is_empty() || env::var_os(key).is_some() { continue; }
            env::set_var(key, parse_env_value(raw_value));
            loaded.push(key.to_string());
        }
    }
    loaded.sort();
    loaded.dedup();
    Ok(loaded)
}

fn config_path(cwd: &Path) -> PathBuf { cwd.join(".jeden/config.json") }
fn auth_path(cwd: &Path) -> PathBuf { cwd.join(".jeden/auth.json") }
fn session_root() -> PathBuf { env::var_os("JEDEN_SESSION_ROOT").map(PathBuf::from).unwrap_or_else(|| dirs_home().join(".jeden/sessions")) }
fn dirs_home() -> PathBuf { env::var_os("HOME").map(PathBuf::from).unwrap_or_else(|| PathBuf::from(".")) }
fn legacy_user_config_path() -> PathBuf { dirs_home().join(".jeden/config.json") }
fn user_config_path() -> PathBuf { dirs_home().join(".jeden/config.yml") }

fn omp_agent_dir() -> PathBuf {
    env::var_os("PI_CODING_AGENT_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| dirs_home().join(".omp/agent"))
}

fn doctor(args: &Args) -> String {
    let config = load_config(&args.cwd);
    let router = agent::model_router_config(&config, args);
    json!({"cwd": args.cwd, "modelRouterUrl": router.url, "agentId": router.agent_id, "model": router.model, "secretPresent": !router.secret.is_empty(), "authFile": auth_path(&args.cwd)}).to_string() + "\n"
}

fn main() -> ExitCode {
    let argv = env::args().skip(usize::from(true)).collect::<Vec<_>>();
    let args = match parse_args(argv) {
        Ok(v) => v,
        Err(e) => { eprintln!("Error: {}\n{}", e, usage()); return ExitCode::FAILURE; }
    };
    if let Err(error) = load_env_files(&args.cwd) {
        eprintln!("Error: failed to load environment files: {}", error);
        return ExitCode::FAILURE;
    }
    let result = match args.command.as_str() {
        "help" => Ok(usage()),
        "interactive" => interactive(&args),
        "run" => agent::run_command(&args),
        "collab-relay" => {
            let addr = args.positionals.first().cloned().unwrap_or_else(|| "127.0.0.1:8877".to_string());
            collab::serve(&addr).map(|_| String::new())
        }
        "sessions" => Ok(list_sessions(args.positionals.first().and_then(|s| s.parse().ok()))),
        "show" => args.positionals.first().map(|id| render_session_export(&read_session_value(id).unwrap_or_else(|e| json!({"error": e})), "json").unwrap_or_default()).ok_or("show requires a session id".into()),
        "export" => export_session_command(&args),
        "artifacts" => args.positionals.first().map(|id| list_artifacts_command(id)).unwrap_or_else(|| Err("artifacts requires a session id".into())),
        "artifact" => artifact_command(&args),
        "tools" => Ok(tools::tools_output(&args.cwd, args.json)),
        "search-sessions" => search_sessions_command(&args),
        "resume" => resume_command(&args),
        "recall_conversation" | "recall-conversation" => recall_conversation_command(&args),
        "update" => update_command(),
        "config" => config_command(&args),
        "doctor" | "capabilities" => Ok(doctor(&args)),
        other => Err(format!("unknown command: {}", other)),
    };
    match result {
        Ok(text) => { print!("{}", text); ExitCode::SUCCESS }
        Err(e) => { eprintln!("Error: {}", e); ExitCode::FAILURE }
    }
}
