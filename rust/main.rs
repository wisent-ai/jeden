use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::cell::RefCell;
use std::collections::BTreeMap;
use std::env;
use std::io::IsTerminal;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

mod tui;
mod agent;
mod mcp;
mod model_router;
mod protocol;
mod slash;
mod tools;
mod tool_runtime;


#[derive(Debug, Clone)]
struct Args {
    command: String,
    cwd: PathBuf,
    model: Option<String>,
    max_tokens: u32,
    max_steps: u32,
    allow_write: bool,
    allow_command: bool,
    json: bool,
    positionals: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
struct Config {
    #[serde(rename = "model")]
    model: Option<String>,
    #[serde(rename = "modelRouterUrl")]
    model_router_url: Option<String>,
    #[serde(rename = "agentId")]
    agent_id: Option<String>,
    #[serde(rename = "authProviders")]
    auth_providers: Option<BTreeMap<String, AuthProviderConfig>>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
struct AuthProviderConfig {
    #[serde(rename = "authUrl")]
    auth_url: Option<String>,
    #[serde(rename = "authorizeUrl")]
    authorize_url: Option<String>,
    #[serde(rename = "tokenUrl")]
    token_url: Option<String>,
    #[serde(rename = "clientId")]
    client_id: Option<String>,
    #[serde(rename = "redirectUri")]
    redirect_uri: Option<String>,
    scope: Option<String>,
    open: Option<bool>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
struct AuthFile {
    #[serde(default)]
    providers: BTreeMap<String, ProviderRecord>,
    #[serde(rename = "activeProvider")]
    active_provider: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
struct ProviderRecord {
    active: bool,
    method: String,
    #[serde(rename = "updatedAt")]
    updated_at: String,
    #[serde(default)]
    oauth: BTreeMap<String, Value>,
    #[serde(default)]
    credentials: BTreeMap<String, Value>,
    #[serde(default)]
    profile: BTreeMap<String, Value>,
}

fn usage() -> &'static str {
    "Usage:\n  jeden [--cwd path] [--model name] [--max-tokens n] [--allow-write] [--allow-command] [--max-steps n]\n  jeden run \"task\" [--json] [--cwd path] [--model name] [--max-tokens n] [--allow-write] [--allow-command] [--max-steps n]\n  jeden sessions [limit]\n  jeden show <session-id-or-path>\n  jeden export <session-id-or-path> [output.json]\n  jeden artifacts <session-id-or-path>\n  jeden artifact <session-id-or-path> <name> [output]\n  jeden config [--cwd path]\n  jeden doctor [--cwd path]\n  jeden capabilities [--cwd path]\n\nSlash commands:\n  /login [provider]      use OMP auth-broker login/provider selection\n  /logout [provider]     use OMP auth-broker logout/provider selection\n  /settings              show auth settings and broker providers\n  /help                  show slash command list\n"
}


fn parse_args(argv: Vec<String>) -> Result<Args, String> {
    let mut rest = argv.into_iter();
    let first = rest.next();
    let mut command = first.unwrap_or_else(|| "interactive".to_string());
    if command == "--help" || command == "-h" { return Ok(Args { command: "help".into(), cwd: env::current_dir().unwrap(), model: None, max_tokens: 2048, max_steps: 8, allow_write: false, allow_command: false, json: false, positionals: vec![] }); }
    if matches!(command.as_str(), "resume" | "recall_conversation" | "recall-conversation" | "search-sessions") {
        return Ok(Args { command, cwd: env::current_dir().map_err(|e| e.to_string())?, model: None, max_tokens: 2048, max_steps: 8, allow_write: false, allow_command: false, json: false, positionals: rest.collect() });
    }
    if command.starts_with("--") { rest = std::iter::once(command).chain(rest).collect::<Vec<_>>().into_iter(); command = "interactive".into(); }
    let mut args = Args { command, cwd: env::current_dir().map_err(|e| e.to_string())?, model: None, max_tokens: 2048, max_steps: 8, allow_write: false, allow_command: false, json: false, positionals: vec![] };
    while let Some(arg) = rest.next() {
        match arg.as_str() {
            "--cwd" => args.cwd = PathBuf::from(rest.next().ok_or("--cwd requires a value")?),
            "--model" => args.model = Some(rest.next().ok_or("--model requires a value")?),
            "--max-tokens" => args.max_tokens = rest.next().ok_or("--max-tokens requires a value")?.parse().map_err(|_| "--max-tokens must be an integer")?,
            "--max-steps" => args.max_steps = rest.next().ok_or("--max-steps requires a value")?.parse().map_err(|_| "--max-steps must be an integer")?,
            "--allow-write" => args.allow_write = true,
            "--allow-command" => args.allow_command = true,
            "--json" => args.json = true,
            other if other.starts_with("--") && (matches!(args.command.as_str(), "export") || (args.command == "run" && !args.positionals.is_empty())) => args.positionals.push(other.to_string()),
            other if other.starts_with("--") => return Err(format!("unknown option: {}", other)),
            other => args.positionals.push(other.to_string()),
        }
    }
    if args.command == "run" && args.positionals.is_empty() { return Err("run requires a task".into()); }
    if args.command == "interactive" && !args.positionals.is_empty() { return Err(format!("unknown command: {}", args.positionals[0])); }
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
    let quoted = (value.starts_with('"') && value.ends_with('"')) || (value.starts_with('\'') && value.ends_with('\''));
    if quoted && value.len() >= 2 {
        value = value[1..value.len() - 1].to_string();
    }
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

fn load_config(cwd: &Path) -> Config {
    let user: Config = read_json(&dirs_home().join(".jeden/config.json"));
    let project: Config = read_json(&config_path(cwd));
    Config {
        model: project.model.or(user.model),
        model_router_url: project.model_router_url.or(user.model_router_url),
        agent_id: project.agent_id.or(user.agent_id),
        auth_providers: project.auth_providers.or(user.auth_providers),
    }
}

fn provider_name(value: &str) -> Option<String> {
    let name = value.trim().to_ascii_lowercase();
    if name.is_empty() { return None; }
    if name.chars().enumerate().all(|(i,c)| c.is_ascii_lowercase() || c.is_ascii_digit() || (i > 0 && matches!(c, '.'|'_'|'-'))) { Some(name) } else { None }
}


fn auth_broker_bin() -> String {
    env::var("JEDEN_AUTH_BROKER_BIN").ok().filter(|value| !value.trim().is_empty()).unwrap_or_else(|| "omp".into())
}

fn auth_broker_output(args: &[&str]) -> Result<String, String> {
    let bin = auth_broker_bin();
    let output = Command::new(&bin)
        .args(args)
        .output()
        .map_err(|error| format!("{} is unavailable: {}", bin, error))?;
    let stdout = String::from_utf8_lossy(&output.stdout).trim().to_string();
    let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
    if output.status.success() {
        return Ok(stdout);
    }
    let details = [stdout, stderr].into_iter().filter(|part| !part.is_empty()).collect::<Vec<_>>().join("\n");
    Err(if details.is_empty() {
        format!("{} {} failed with {}", bin, args.join(" "), output.status)
    } else {
        format!("{} {} failed with {}\n{}", bin, args.join(" "), output.status, details)
    })
}

fn broker_provider_ids() -> Result<Vec<String>, String> {
    let text = auth_broker_output(&["auth-broker", "list", "--json"])?;
    let value: Value = serde_json::from_str(&text).map_err(|error| format!("invalid auth-broker provider list: {}", error))?;
    Ok(value.as_array().map(|providers| {
        providers.iter().filter_map(|provider| provider.get("id").and_then(Value::as_str).map(str::to_string)).collect()
    }).unwrap_or_default())
}

fn broker_provider_summary() -> String {
    match auth_broker_output(&["auth-broker", "list", "--json"]) {
        Ok(text) => {
            let value: Value = serde_json::from_str(&text).unwrap_or(Value::Null);
            let providers = value.as_array().cloned().unwrap_or_default();
            let names = providers.iter().filter_map(|provider| {
                let id = provider.get("id").and_then(Value::as_str)?;
                let name = provider.get("name").and_then(Value::as_str).unwrap_or(id);
                Some(format!("{} ({})", id, name))
            }).take(12).collect::<Vec<_>>();
            let suffix = if providers.len() > names.len() { format!("; +{} more", providers.len() - names.len()) } else { String::new() };
            format!("OMP auth-broker providers: {}{}.", names.join(", "), suffix)
        }
        Err(error) => format!("OMP auth-broker providers: unavailable ({})", error.lines().next().unwrap_or("unknown error")),
    }
}

fn format_auth_status(cwd: &Path) -> String {
    let auth: AuthFile = read_json(&auth_path(cwd));
    let mut out = vec![
        "Jeden provider/auth settings".to_string(),
        format!("Workspace: {}", cwd.display()),
        format!("Auth backend: {} auth-broker", auth_broker_bin()),
        broker_provider_summary(),
        format!("Legacy auth file: {}", auth_path(cwd).display()),
    ];
    if auth.providers.is_empty() { out.push("Legacy configured providers: none".into()); } else { out.push(format!("Legacy configured providers ({})", auth.providers.len())); }
    for (name, record) in auth.providers { out.push(format!("- {}{}", name, if record.active { " (active)" } else { "" })); out.push(format!("  method: {}", record.method)); out.push(format!("  credentials: {} key(s)", record.credentials.len())); }
    out.push("".into());
    out.push("Actions:".into());
    out.push("  /login                                      open OMP auth-broker provider selection".into());
    out.push("  /login <provider>                           run OMP auth-broker login for a provider".into());
    out.push("  /logout [provider]                          run OMP auth-broker logout".into());
    out.join("\n")
}


fn auth_broker_command(action: &str, args: &[&str]) -> Result<String, String> {
    let mut broker_args = vec!["auth-broker", action];
    broker_args.extend_from_slice(args);
    let should_capture = !std::io::stdin().is_terminal() || args.iter().any(|arg| *arg == "--dry-run" || *arg == "--json");
    if should_capture {
        return auth_broker_output(&broker_args);
    }

    let bin = auth_broker_bin();
    let status = Command::new(&bin)
        .args(&broker_args)
        .status()
        .map_err(|error| format!("{} auth-broker is unavailable: {}", bin, error))?;
    if status.success() {
        Ok(format!("{} auth-broker {} completed.", bin, action))
    } else {
        Err(format!("{} auth-broker {} failed with {}.", bin, action, status))
    }
}

fn parse_broker_args(raw: &str, allow_empty_provider: bool) -> Result<Vec<&str>, String> {
    let mut out = Vec::new();
    let mut saw_provider = false;
    for token in raw.split_whitespace() {
        if token.starts_with("--") {
            if matches!(token, "--json" | "--dry-run") || token.starts_with("--via=") {
                out.push(token);
                continue;
            }
            return Err(format!("Unsupported auth-broker flag for slash command: {}", token));
        }
        let name = provider_name(token).ok_or("invalid provider")?;
        if saw_provider {
            return Err("Only one auth provider may be supplied.".into());
        }
        saw_provider = true;
        out.push(token);
        if name != token {
            return Err("Auth provider ids must be lowercase ASCII ids.".into());
        }
    }
    if !allow_empty_provider && !saw_provider {
        return Err("Usage: /logout <provider>".into());
    }
    Ok(out)
}

fn start_login(_cwd: &Path, args: &str) -> Result<String, String> {
    let tokens = args.split_whitespace().collect::<Vec<_>>();
    let provider_tokens = tokens.iter().copied().filter(|token| !token.starts_with("--")).collect::<Vec<_>>();
    if provider_tokens.is_empty() {
        let broker_args = parse_broker_args(args, true)?;
        return auth_broker_command("login", &broker_args);
    }
    if provider_tokens.len() != 1 {
        return Ok("Warning: No OAuth login is waiting for a manual callback.".into());
    }
    let candidate = provider_tokens[0];
    let Some(name) = provider_name(candidate) else {
        return Ok("Warning: No OAuth login is waiting for a manual callback.".into());
    };
    let providers = match broker_provider_ids() {
        Ok(providers) if !providers.is_empty() => providers,
        _ => {
            let broker_args = parse_broker_args(args, true)?;
            return auth_broker_command("login", &broker_args);
        }
    };
    if !providers.iter().any(|provider| provider == &name) {
        return Ok("Warning: No OAuth login is waiting for a manual callback.".into());
    }
    let broker_args = parse_broker_args(args, true)?;
    auth_broker_command("login", &broker_args)
}

fn logout(_cwd: &Path, args: &str) -> Result<String, String> {
    let broker_args = parse_broker_args(args, true)?;
    auth_broker_command("logout", &broker_args)
}


const SLASH_COMMANDS: &[(&str, &str)] = &[
    ("settings", "Open settings menu"), ("setup", "Open provider setup"), ("plan", "Toggle plan mode"),
    ("plan-review", "Review latest plan"), ("goal", "Toggle goal mode"), ("loop", "Toggle loop mode"), ("model", "Switch model"),
    ("fast", "Toggle priority service tier"), ("advisor", "Toggle advisor reviewer"),
    ("export", "Export session"), ("dump", "Dump session"), ("share", "Share session"),
    ("collab", "Collaborate via relay"), ("join", "Join shared session"), ("leave", "Leave collab"),
    ("browser", "Configure browser runtime"), ("copy", "Copy conversation text"), ("todo", "Manage todos"),
    ("session", "Session management"), ("jobs", "Show jobs"), ("usage", "Show provider usage"),
    ("stats", "Launch stats dashboard"), ("changelog", "Show changelog"), ("hotkeys", "Show hotkeys"),
    ("tools", "Show tools"), ("context", "Show context usage"), ("extensions", "Manage extensions"),
    ("agents", "Agent controls"), ("branch", "Create branch"), ("fork", "Create fork"), ("tree", "Navigate tree"),
    ("login", "Automated OAuth login"), ("logout", "Logout provider"), ("mcp", "Manage MCP servers"),
    ("ssh", "Manage SSH hosts"), ("new", "Start new session"), ("fresh", "Reset provider stream state"),
    ("drop", "Drop current session"), ("compact", "Compact session"), ("shake", "Shake session context"),
    ("handoff", "Hand off session"), ("resume", "Resume session"), ("btw", "Side question"),
    ("tan", "Background agent"), ("omfg", "Forge local rule"), ("retry", "Retry last failed turn"),
    ("debug", "Open debug tools"), ("memory", "Memory maintenance"), ("rename", "Rename session"),
    ("move", "Move session workspace"), ("marketplace", "Manage marketplace plugins"),
    ("plugins", "Manage installed plugins"), ("reload-plugins", "Reload plugins"),
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

fn update_tool(env_key: &str, default: &str) -> String {
    #[cfg(test)]
    {
        return env::var(env_key).ok().filter(|value| !value.trim().is_empty()).unwrap_or_else(|| default.into());
    }
    #[cfg(not(test))]
    {
        let _ = env_key;
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

fn update_command() -> Result<String, String> {
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
        "/help" | "/commands" => Ok(format_slash_help()),
        "/settings" | "/setup" | "/providers" => Ok(format_auth_status(cwd)),
        "/login" => start_login(cwd, parts.collect::<Vec<_>>().join(" ").as_str()),
        "/logout" => logout(cwd, parts.collect::<Vec<_>>().join(" ").as_str()),
        "/model" | "/models" | "/switch" => handle_model_slash(cwd, model, parts.collect::<Vec<_>>().join(" ").as_str()),
        "/usage" => Ok(crate::slash::handle_local(&slash_context, trimmed).transpose()?.unwrap_or_else(|| "Usage accounting is available in Rust mode-state.".into())),
        "/update" => update_command(),
        "/exit" | "/quit" => Ok("Exit is handled by the interactive input loop.".into()),
        _ => Err(format!("Unknown Rust slash command: {}", command)),
    }
}

fn list_sessions(limit: usize) -> String {
    let mut rows = vec![];
    if let Ok(entries) = fs::read_dir(session_root()) {
        for entry in entries.flatten().take(limit) { rows.push(entry.file_name().to_string_lossy().to_string()); }
    }
    if rows.is_empty() { "No sessions found.\n".into() } else { rows.join("\n") + "\n" }
}

fn search_sessions_command(args: &Args) -> Result<String, String> {
    let query = args.positionals.get(0).ok_or("search-sessions requires a query")?.trim().to_ascii_lowercase();
    if query.is_empty() { return Err("search-sessions requires a non-empty query".into()); }
    let limit = args.positionals.get(1).and_then(|value| value.parse::<usize>().ok()).unwrap_or(50).clamp(1, 200);
    let mut rows = Vec::new();
    if let Ok(entries) = fs::read_dir(session_root()) {
        let mut entries = entries.flatten().map(|entry| entry.path()).collect::<Vec<_>>();
        entries.sort_by(|a, b| b.file_name().cmp(&a.file_name()));
        for dir in entries.into_iter().take(limit) {
            let session = match read_session_value(&dir.display().to_string()) {
                Ok(session) => session,
                Err(_) => continue,
            };
            let id = session.get("id").and_then(Value::as_str).unwrap_or("");
            let events = session.get("events").and_then(Value::as_array).cloned().unwrap_or_default();
            for event in events {
                let text = serde_json::to_string(event.get("data").unwrap_or(&Value::Null)).unwrap_or_default();
                let lower = text.to_ascii_lowercase();
                let Some(at) = lower.find(&query) else { continue; };
                let char_at = lower[..at].chars().count();
                let take = query.chars().count() + 240;
                let snippet = text
                    .chars()
                    .skip(char_at.saturating_sub(80))
                    .take(take)
                    .collect::<String>()
                    .split_whitespace()
                    .collect::<Vec<_>>()
                    .join(" ");
                rows.push(format!("{}\t{}\t{}\t{}", id, event.get("ts").and_then(Value::as_str).unwrap_or(""), event.get("type").and_then(Value::as_str).unwrap_or(""), snippet));
                break;
            }
        }
    }
    Ok(if rows.is_empty() { String::new() } else { rows.join("\n") + "\n" })
}



fn session_dir_for(id_or_path: &str) -> PathBuf {
    if id_or_path.contains('/') { PathBuf::from(id_or_path) } else { session_root().join(id_or_path) }
}

fn read_transcript_events(dir: &Path) -> Vec<Value> {
    let file = dir.join("transcript.jsonl");
    fs::read_to_string(file).unwrap_or_default().lines().filter_map(|line| serde_json::from_str::<Value>(line).ok()).collect()
}

fn read_session_value(id_or_path: &str) -> Result<Value, String> {
    let dir = session_dir_for(id_or_path);
    if !dir.exists() { return Err(format!("session not found: {}", dir.display())); }
    let state: Value = read_json(&dir.join("state.json"));
    let id = dir.file_name().map(|v| v.to_string_lossy().to_string()).unwrap_or_else(|| id_or_path.to_string());
    Ok(json!({"id": id, "path": dir, "state": state, "events": read_transcript_events(&dir)}))
}

fn html_escape(value: &str) -> String {
    value.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;").replace('"', "&quot;")
}

fn render_session_export(session: &Value, format: &str) -> Result<String, String> {
    if format == "json" {
        return Ok(serde_json::to_string_pretty(session).map_err(|e| e.to_string())? + "\n");
    }
    let id = session.get("id").and_then(Value::as_str).unwrap_or("session");
    let path = session.get("path").and_then(Value::as_str).unwrap_or("");
    let events = session.get("events").and_then(Value::as_array).cloned().unwrap_or_default();
    if format == "markdown" || format == "md" {
        let mut out = format!("# Jeden session {}\n\n{}\n\n", id, path);
        for event in events {
            let label = format!("{} {}", event.get("ts").and_then(Value::as_str).unwrap_or(""), event.get("type").and_then(Value::as_str).unwrap_or("")).trim().to_string();
            let data = serde_json::to_string_pretty(event.get("data").unwrap_or(&Value::Null)).unwrap_or_else(|_| "{}".into());
            out.push_str(&format!("## {}\n\n```json\n{}\n```\n\n", label, data));
        }
        return Ok(out);
    }
    if format == "html" {
        let mut sections = String::new();
        for event in events {
            let label = html_escape(format!("{} {}", event.get("ts").and_then(Value::as_str).unwrap_or(""), event.get("type").and_then(Value::as_str).unwrap_or("")).trim());
            let body = html_escape(&serde_json::to_string_pretty(event.get("data").unwrap_or(&Value::Null)).unwrap_or_else(|_| "{}".into()));
            sections.push_str(&format!("<section class=\"event\"><h2>{}</h2><pre>{}</pre></section>\n", label, body));
        }
        return Ok(format!("<!doctype html>\n<html><head><meta charset=\"utf-8\"><title>Jeden session {}</title><style>body{{font-family:ui-sans-serif,system-ui,sans-serif;margin:2rem;background:#fafafa;color:#111}}.event{{border:1px solid #ddd;border-radius:8px;background:white;margin:1rem 0;padding:1rem}}pre{{white-space:pre-wrap;overflow-wrap:anywhere}}</style></head><body><h1>Jeden session {}</h1><p>{}</p>{}</body></html>\n", html_escape(id), html_escape(id), html_escape(path), sections));
    }
    Err(format!("unsupported session export format: {}", format))
}

fn export_session_command(args: &Args) -> Result<String, String> {
    let id = args.positionals.get(0).ok_or("export requires a session id or path")?;
    let mut format = "json".to_string();
    let mut output = None;
    for arg in args.positionals.iter().skip(1) {
        if arg == "--html" { format = "html".into(); }
        else if arg == "--markdown" { format = "markdown".into(); }
        else { output = Some(arg.clone()); }
    }
    let payload = render_session_export(&read_session_value(id)?, &format)?;
    if let Some(path) = output { fs::write(&path, &payload).map_err(|e| e.to_string())?; Ok(format!("{}\n", path)) } else { Ok(payload) }
}

fn list_artifacts_command(id_or_path: &str) -> Result<String, String> {
    let dir = session_dir_for(id_or_path).join("artifacts");
    let mut rows = vec![];
    if let Ok(entries) = fs::read_dir(dir) {
        for entry in entries.flatten() {
            if let Ok(meta) = entry.metadata() {
                if meta.is_file() { rows.push(format!("{}\t{}", entry.file_name().to_string_lossy(), meta.len())); }
            }
        }
    }
    rows.sort();
    Ok(if rows.is_empty() { String::new() } else { rows.join("\n") + "\n" })
}

fn artifact_command(args: &Args) -> Result<String, String> {
    let id = args.positionals.get(0).ok_or("artifact requires a session id or path")?;
    let name = args.positionals.get(1).ok_or("artifact requires an artifact name")?;
    let root = session_dir_for(id).join("artifacts");
    let file = root.join(name);
    let canonical_root = fs::canonicalize(&root).map_err(|e| e.to_string())?;
    let canonical_file = fs::canonicalize(&file).map_err(|e| e.to_string())?;
    if !canonical_file.starts_with(&canonical_root) { return Err(format!("artifact path escapes session: {}", name)); }
    let content = fs::read_to_string(&canonical_file).map_err(|e| e.to_string())?;
    if let Some(output) = args.positionals.get(2) { fs::write(output, &content).map_err(|e| e.to_string())?; Ok(format!("{}\n", output)) } else { Ok(if content.ends_with('\n') { content } else { content + "\n" }) }
}


fn doctor(args: &Args) -> String {
    let config = load_config(&args.cwd);
    let router = agent::model_router_config(&config, args);
    json!({"cwd": args.cwd, "modelRouterUrl": router.url, "agentId": router.agent_id, "model": router.model, "secretPresent": !router.secret.is_empty(), "authFile": auth_path(&args.cwd)}).to_string() + "\n"
}



fn git_prompt_status(cwd: &Path) -> (Option<String>, usize) {
    let branch = Command::new("git")
        .arg("-C")
        .arg(cwd)
        .args(["branch", "--show-current"])
        .output()
        .ok()
        .filter(|output| output.status.success())
        .map(|output| String::from_utf8_lossy(&output.stdout).trim().to_string())
        .filter(|value| !value.is_empty());
    let dirty_count = Command::new("git")
        .arg("-C")
        .arg(cwd)
        .args(["status", "--porcelain"])
        .output()
        .ok()
        .filter(|output| output.status.success())
        .map(|output| String::from_utf8_lossy(&output.stdout).lines().count())
        .unwrap_or(0);
    (branch, dirty_count)
}

fn service_tier_prompt(cwd: &Path) -> String {
    let mode: Value = read_json(&cwd.join(".jeden/mode-state.json"));
    if mode.pointer("/fast/enabled").and_then(Value::as_bool).unwrap_or(false) {
        if let Some(tier) = mode.pointer("/fast/serviceTier").and_then(Value::as_str).filter(|value| !value.trim().is_empty()) {
            return tier.to_string();
        }
    }
    env::var("JEDEN_SERVICE_TIER")
        .ok()
        .or_else(|| env::var("MODEL_SERVICE_TIER").ok())
        .filter(|value| !value.trim().is_empty())
        .unwrap_or_else(|| "default".into())
}
fn interactive(args: &Args) -> Result<String, String> {
    let config = load_config(&args.cwd);
    let model = args
        .model
        .clone()
        .or(config.model)
        .or_else(|| env::var("JEDEN_MODEL").ok())
        .or_else(|| env::var("MODEL").ok())
        .unwrap_or_else(|| "default".into());
    let session_model = RefCell::new(Some(model));
    tui::run_basic_loop(
        || {
            let (branch, dirty_count) = git_prompt_status(&args.cwd);
            tui::PromptStatus {
                cwd: args.cwd.display().to_string(),
                write_status: if args.allow_write { "allow".into() } else { "ask".into() },
                command_status: if args.allow_command { "allow".into() } else { "ask".into() },
                model: session_model.borrow().clone().unwrap_or_else(|| "default".into()),
                service_tier: service_tier_prompt(&args.cwd),
                branch,
                dirty_count,
                context_percent: None,
                context_limit: None,
                cost: None,
            }
        },
        |input| {
            if input.trim_start().starts_with('/') {
                let trimmed = input.trim();
                let (command, rest) = trimmed.split_once(char::is_whitespace).unwrap_or((trimmed, ""));
                if matches!(command, "/model" | "/models" | "/switch") {
                    let next = rest.trim();
                    if next.is_empty() {
                        return Ok(format!("Current model route: {}.", session_model.borrow().as_deref().unwrap_or("default")));
                    }
                    *session_model.borrow_mut() = Some(next.to_string());
                    return Ok(format!("Model route set to {}.", next));
                }
                if command == "/retry" {
                    let mut run_args = args.clone();
                    run_args.command = "run".into();
                    run_args.model = session_model.borrow().clone();
                    run_args.positionals = vec![trimmed.to_string()];
                    run_args.json = false;
                    return agent::retry_command(&run_args).map(|text| text.trim().to_string());
                }
                if command == "/btw" {
                    let mut run_args = args.clone();
                    run_args.command = "run".into();
                    run_args.model = session_model.borrow().clone();
                    run_args.positionals = vec![trimmed.to_string()];
                    run_args.json = false;
                    return agent::btw_command(&run_args, rest).map(|text| text.trim().to_string());
                }
                handle_slash(&args.cwd, input, session_model.borrow().as_deref())
            } else {
                let mut run_args = args.clone();
                run_args.command = "run".into();
                run_args.model = session_model.borrow().clone();
                run_args.positionals = vec![input.to_string()];
                run_args.json = false;
                agent::run_command(&run_args).map(|text| text.trim().to_string())
            }
        },
    )
    .map_err(|e| e.to_string())?;
    Ok(String::new())
}


fn main() {
    let argv = env::args().skip(1).collect::<Vec<_>>();
    let args = match parse_args(argv) { Ok(v) => v, Err(e) => { eprintln!("Error: {}\n{}", e, usage()); std::process::exit(2); } };
    if let Err(error) = load_env_files(&args.cwd) {
        eprintln!("Error: failed to load environment files: {}", error);
        std::process::exit(1);
    }
    let result = match args.command.as_str() {
        "help" => Ok(usage().to_string()),
        "interactive" => interactive(&args),
        "run" => agent::run_command(&args),
        "sessions" => Ok(list_sessions(args.positionals.get(0).and_then(|s| s.parse().ok()).unwrap_or(20))),
        "show" => args.positionals.get(0).map(|id| render_session_export(&read_session_value(id).unwrap_or_else(|e| json!({"error": e})), "json").unwrap_or_default()).ok_or("show requires a session id".into()),
        "export" => export_session_command(&args),
        "artifacts" => args.positionals.get(0).map(|id| list_artifacts_command(id)).unwrap_or_else(|| Err("artifacts requires a session id".into())),
        "artifact" => artifact_command(&args),
        "tools" => Ok(tools::tools_table(&args.cwd)),
        "search-sessions" => search_sessions_command(&args),
        "resume" | "recall_conversation" | "recall-conversation" => Err(format!("{} is not available in the Rust CLI yet; use sessions/show/export/search-sessions for recorded session inspection.", args.command)),
        "update" => update_command(),
        "config" => Ok(serde_json::to_string_pretty(&load_config(&args.cwd)).unwrap() + "\n"),
        "doctor" | "capabilities" => Ok(doctor(&args)),
        other => Err(format!("unknown command: {}", other)),
    };
    match result { Ok(text) => print!("{}", text), Err(e) => { eprintln!("Error: {}", e); std::process::exit(1); } }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::ffi::OsString;
    #[cfg(unix)]
    use std::os::unix::fs::PermissionsExt;
    use std::sync::{Mutex, OnceLock};

    fn temp_workspace(name: &str) -> PathBuf {
        let path = env::temp_dir().join(format!("jeden-main-{}-{}-{}", name, std::process::id(), now_millis_for_test()));
        fs::create_dir_all(&path).unwrap();
        path
    }

    fn now_millis_for_test() -> u128 {
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis()
    }

    fn env_lock() -> &'static Mutex<()> {
        static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
        LOCK.get_or_init(|| Mutex::new(()))
    }

    struct EnvVarGuard {
        key: &'static str,
        previous: Option<OsString>,
    }

    impl EnvVarGuard {
        fn set(key: &'static str, value: impl AsRef<std::ffi::OsStr>) -> Self {
            let previous = env::var_os(key);
            env::set_var(key, value);
            Self { key, previous }
        }
    }

    impl Drop for EnvVarGuard {
        fn drop(&mut self) {
            if let Some(previous) = &self.previous {
                env::set_var(self.key, previous);
            } else {
                env::remove_var(self.key);
            }
        }
    }

    #[cfg(unix)]
    fn fake_auth_broker_bin_with_script(cwd: &Path, script: &str) -> PathBuf {
        let bin = cwd.join("fake-omp");
        fs::write(&bin, script).unwrap();
        let mut permissions = fs::metadata(&bin).unwrap().permissions();
        permissions.set_mode(0o755);
        fs::set_permissions(&bin, permissions).unwrap();
        bin
    }

    #[cfg(unix)]
    fn fake_auth_broker_bin(cwd: &Path) -> PathBuf {
        fake_auth_broker_bin_with_script(
            cwd,
            r#"#!/bin/sh
printf '%s\n' "$*" >> "$JEDEN_TEST_BROKER_LOG"
if [ "$1" = "auth-broker" ] && [ "$2" = "list" ] && [ "$3" = "--json" ]; then
  printf '%s\n' '[{"id":"github","name":"GitHub"},{"id":"azure","name":"Azure Foundry"}]'
  exit 0
fi
printf '%s\n' "unexpected broker command: $*" >&2
exit 64
"#,
        )
    }

    #[cfg(unix)]
    fn fake_update_tool_bin(cwd: &Path, name: &str, script: &str) -> PathBuf {
        let bin = cwd.join(name);
        fs::write(&bin, script).unwrap();
        let mut permissions = fs::metadata(&bin).unwrap().permissions();
        permissions.set_mode(0o755);
        fs::set_permissions(&bin, permissions).unwrap();
        bin
    }

    #[cfg(unix)]
    struct UpdateToolFixture {
        log: PathBuf,
        _git_env: EnvVarGuard,
        _cargo_env: EnvVarGuard,
        _log_env: EnvVarGuard,
    }

    #[cfg(unix)]
    fn install_fake_update_tools(cwd: &Path, git_script: &str, cargo_script: &str) -> UpdateToolFixture {
        let git = fake_update_tool_bin(cwd, "fake-git", git_script);
        let cargo = fake_update_tool_bin(cwd, "fake-cargo", cargo_script);
        let log = cwd.join("update.log");
        UpdateToolFixture {
            log: log.clone(),
            _git_env: EnvVarGuard::set("JEDEN_UPDATE_GIT_BIN", &git),
            _cargo_env: EnvVarGuard::set("JEDEN_UPDATE_CARGO_BIN", &cargo),
            _log_env: EnvVarGuard::set("JEDEN_TEST_UPDATE_LOG", &log),
        }
    }

    #[test]
    #[cfg(unix)]
    fn slash_parity_login_unknown_provider_uses_pending_callback_warning_with_broker_list() {
        let _env_guard = env_lock().lock().unwrap();
        let cwd = temp_workspace("login");
        let broker = fake_auth_broker_bin(&cwd);
        let log = cwd.join("broker.log");
        let _broker_env = EnvVarGuard::set("JEDEN_AUTH_BROKER_BIN", &broker);
        let _log_env = EnvVarGuard::set("JEDEN_TEST_BROKER_LOG", &log);

        let result = handle_slash(&cwd, "/login bogus", None).unwrap();

        assert_eq!(result, "Warning: No OAuth login is waiting for a manual callback.");
        assert!(!result.to_ascii_lowercase().contains("wisent"));
        assert_eq!(fs::read_to_string(&log).unwrap(), "auth-broker list --json\n");
    }

    #[test]
    #[cfg(unix)]
    fn slash_parity_login_known_provider_returns_auth_broker_success_output() {
        let _env_guard = env_lock().lock().unwrap();
        let cwd = temp_workspace("login-known-provider");
        let broker = fake_auth_broker_bin_with_script(
            &cwd,
            r#"#!/bin/sh
printf '%s\n' "$*" >> "$JEDEN_TEST_BROKER_LOG"
if [ "$1" = "auth-broker" ] && [ "$2" = "list" ] && [ "$3" = "--json" ]; then
  printf '%s\n' '[{"id":"github","name":"GitHub"},{"id":"azure","name":"Azure Foundry"}]'
  exit 0
fi
if [ "$1" = "auth-broker" ] && [ "$2" = "login" ] && [ "$3" = "github" ]; then
  printf '%s\n' 'broker login succeeded for github'
  exit 0
fi
printf '%s\n' "unexpected broker command: $*" >&2
exit 64
"#,
        );
        let log = cwd.join("broker.log");
        let _broker_env = EnvVarGuard::set("JEDEN_AUTH_BROKER_BIN", &broker);
        let _log_env = EnvVarGuard::set("JEDEN_TEST_BROKER_LOG", &log);

        let result = handle_slash(&cwd, "/login github", None).unwrap();

        assert_eq!(result, "broker login succeeded for github");
        assert_eq!(fs::read_to_string(&log).unwrap(), "auth-broker list --json\nauth-broker login github\n");
    }

    #[test]
    #[cfg(unix)]
    fn slash_parity_login_provider_shaped_arg_falls_back_when_broker_list_is_unavailable_or_malformed() {
        let _env_guard = env_lock().lock().unwrap();
        let cases = [
            (
                "login-list-unavailable",
                r#"#!/bin/sh
printf '%s\n' "$*" >> "$JEDEN_TEST_BROKER_LOG"
if [ "$1" = "auth-broker" ] && [ "$2" = "list" ] && [ "$3" = "--json" ]; then
  printf '%s\n' 'provider list unavailable' >&2
  exit 70
fi
if [ "$1" = "auth-broker" ] && [ "$2" = "login" ] && [ "$3" = "github" ]; then
  printf '%s\n' 'fallback login succeeded after unavailable list'
  exit 0
fi
printf '%s\n' "unexpected broker command: $*" >&2
exit 64
"#,
                "fallback login succeeded after unavailable list",
            ),
            (
                "login-list-malformed",
                r#"#!/bin/sh
printf '%s\n' "$*" >> "$JEDEN_TEST_BROKER_LOG"
if [ "$1" = "auth-broker" ] && [ "$2" = "list" ] && [ "$3" = "--json" ]; then
  printf '%s\n' 'not-json'
  exit 0
fi
if [ "$1" = "auth-broker" ] && [ "$2" = "login" ] && [ "$3" = "github" ]; then
  printf '%s\n' 'fallback login succeeded after malformed list'
  exit 0
fi
printf '%s\n' "unexpected broker command: $*" >&2
exit 64
"#,
                "fallback login succeeded after malformed list",
            ),
        ];

        for (name, script, expected) in cases {
            let cwd = temp_workspace(name);
            let broker = fake_auth_broker_bin_with_script(&cwd, script);
            let log = cwd.join("broker.log");
            let _broker_env = EnvVarGuard::set("JEDEN_AUTH_BROKER_BIN", &broker);
            let _log_env = EnvVarGuard::set("JEDEN_TEST_BROKER_LOG", &log);

            let result = handle_slash(&cwd, "/login github", None).unwrap();

            assert_eq!(result, expected, "{name}");
            assert_eq!(fs::read_to_string(&log).unwrap(), "auth-broker list --json\nauth-broker login github\n", "{name}");
        }
    }

    #[test]
    #[cfg(unix)]
    fn slash_parity_settings_reports_auth_broker_provider_summary() {
        let _env_guard = env_lock().lock().unwrap();
        let cwd = temp_workspace("settings");
        let broker = fake_auth_broker_bin(&cwd);
        let log = cwd.join("broker.log");
        let _broker_env = EnvVarGuard::set("JEDEN_AUTH_BROKER_BIN", &broker);
        let _log_env = EnvVarGuard::set("JEDEN_TEST_BROKER_LOG", &log);

        let result = handle_slash(&cwd, "/settings", None).unwrap();

        assert!(result.contains(&format!("Auth backend: {} auth-broker", broker.display())));
        assert!(result.contains("OMP auth-broker providers: github (GitHub), azure (Azure Foundry)."));
        assert!(result.contains("Legacy configured providers: none"));
        assert!(result.contains("/login                                      open OMP auth-broker provider selection"));
        assert_eq!(fs::read_to_string(&log).unwrap(), "auth-broker list --json\n");
    }

    #[test]
    #[cfg(unix)]
    fn update_command_runs_git_pull_and_release_build_without_manual_instructions() {
        let _env_guard = env_lock().lock().unwrap();
        let cwd = temp_workspace("update-command");
        let fixture = install_fake_update_tools(
            &cwd,
            r#"#!/bin/sh
printf 'git %s\n' "$*" >> "$JEDEN_TEST_UPDATE_LOG"
if [ "$1" = "rev-parse" ] && [ "$2" = "--short" ] && [ "$3" = "HEAD" ]; then
  printf '%s\n' 'abc123'
  exit 0
fi
if [ "$1" = "pull" ] && [ "$2" = "--ff-only" ]; then
  printf '%s\n' 'pulled fast-forward changes'
  exit 0
fi
printf '%s\n' "unexpected git command: $*" >&2
exit 64
"#,
            r#"#!/bin/sh
printf 'cargo %s\n' "$*" >> "$JEDEN_TEST_UPDATE_LOG"
if [ "$1" = "build" ] && [ "$2" = "--release" ]; then
  printf '%s\n' 'compiled release binary'
  exit 0
fi
printf '%s\n' "unexpected cargo command: $*" >&2
exit 64
"#,
        );

        let result = update_command().unwrap();

        assert!(result.contains("Jeden update completed"), "{result}");
        assert!(result.contains("git pull --ff-only:\npulled fast-forward changes"), "{result}");
        assert!(result.contains("cargo build --release:\ncompiled release binary"), "{result}");
        for manual_instruction in ["To update this source install", "npm link", "rehash"] {
            assert!(!result.contains(manual_instruction), "{manual_instruction} leaked into update output:\n{result}");
        }
        assert_eq!(
            fs::read_to_string(&fixture.log).unwrap(),
            "git rev-parse --short HEAD\ngit pull --ff-only\ncargo build --release\ngit rev-parse --short HEAD\n"
        );
    }

    #[test]
    #[cfg(unix)]
    fn slash_update_routes_to_automated_update_flow_without_manual_instructions() {
        let _env_guard = env_lock().lock().unwrap();
        let cwd = temp_workspace("slash-update");
        let fixture = install_fake_update_tools(
            &cwd,
            r#"#!/bin/sh
printf 'git %s\n' "$*" >> "$JEDEN_TEST_UPDATE_LOG"
if [ "$1" = "rev-parse" ] && [ "$2" = "--short" ] && [ "$3" = "HEAD" ]; then
  printf '%s\n' 'slash-head'
  exit 0
fi
if [ "$1" = "pull" ] && [ "$2" = "--ff-only" ]; then
  printf '%s\n' 'slash pull completed'
  exit 0
fi
printf '%s\n' "unexpected git command: $*" >&2
exit 64
"#,
            r#"#!/bin/sh
printf 'cargo %s\n' "$*" >> "$JEDEN_TEST_UPDATE_LOG"
if [ "$1" = "build" ] && [ "$2" = "--release" ]; then
  printf '%s\n' 'slash release build completed'
  exit 0
fi
printf '%s\n' "unexpected cargo command: $*" >&2
exit 64
"#,
        );

        let result = handle_slash(&cwd, "/update", None).unwrap();

        assert!(result.contains("Jeden update completed"), "{result}");
        assert!(result.contains("slash pull completed"), "{result}");
        assert!(result.contains("slash release build completed"), "{result}");
        for manual_instruction in ["To update this source install", "npm link", "rehash"] {
            assert!(!result.contains(manual_instruction), "{manual_instruction} leaked into /update output:\n{result}");
        }
        assert_eq!(
            fs::read_to_string(&fixture.log).unwrap(),
            "git rev-parse --short HEAD\ngit pull --ff-only\ncargo build --release\ngit rev-parse --short HEAD\n"
        );
    }

    #[test]
    #[cfg(unix)]
    fn update_command_reports_git_pull_failure_and_stops_before_build() {
        let _env_guard = env_lock().lock().unwrap();
        let cwd = temp_workspace("update-pull-failure");
        let fixture = install_fake_update_tools(
            &cwd,
            r#"#!/bin/sh
printf 'git %s\n' "$*" >> "$JEDEN_TEST_UPDATE_LOG"
if [ "$1" = "rev-parse" ] && [ "$2" = "--short" ] && [ "$3" = "HEAD" ]; then
  printf '%s\n' 'before-failure'
  exit 0
fi
if [ "$1" = "pull" ] && [ "$2" = "--ff-only" ]; then
  printf '%s\n' 'network rejected fast-forward' >&2
  exit 42
fi
printf '%s\n' "unexpected git command: $*" >&2
exit 64
"#,
            r#"#!/bin/sh
printf 'cargo %s\n' "$*" >> "$JEDEN_TEST_UPDATE_LOG"
printf '%s\n' "cargo should not run after pull failure" >&2
exit 64
"#,
        );

        let error = update_command().unwrap_err();

        assert!(error.contains("git pull --ff-only failed with exit status: 42"), "{error}");
        assert!(error.contains("network rejected fast-forward"), "{error}");
        assert_eq!(
            fs::read_to_string(&fixture.log).unwrap(),
            "git rev-parse --short HEAD\ngit pull --ff-only\n"
        );
    }

    #[test]
    fn slash_parity_model_selection_persists_project_config() {
        let cwd = temp_workspace("model");
        fs::create_dir_all(cwd.join(".jeden")).unwrap();
        fs::write(cwd.join(".jeden/config.json"), r#"{"agentId":"keep-me"}"#).unwrap();

        let result = handle_slash(&cwd, "/model claude-opus-4-1", None).unwrap();
        let config: Value = read_json(&cwd.join(".jeden/config.json"));

        assert_eq!(result, "Model route set to claude-opus-4-1.");
        assert_eq!(config["model"], "claude-opus-4-1");
        assert_eq!(config["agentId"], "keep-me");
    }

    #[test]
    fn slash_parity_plan_review_reports_inactive_and_missing_plan() {
        let cwd = temp_workspace("plan-review");

        let inactive = handle_slash(&cwd, "/plan-review", None).unwrap();
        assert_eq!(inactive, "Warning: Plan mode is not active.");

        let enabled = handle_slash(&cwd, "/plan on", None).unwrap();
        assert_eq!(enabled, "Plan mode enabled.");

        let missing_plan = handle_slash(&cwd, "/plan-review", None).unwrap();
        assert_eq!(missing_plan, "No plan review is available yet.");
    }
}
