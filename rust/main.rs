use rand::{distributions::Alphanumeric, Rng};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::BTreeMap;
use std::env;
use std::fs;
use std::io::{Read, Write};
use std::net::TcpListener;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use url::Url;

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
    "Usage:\n  jeden [--cwd path] [--model name] [--max-tokens n] [--allow-write] [--allow-command] [--max-steps n]\n  jeden run \"task\" [--json] [--cwd path] [--model name] [--max-tokens n] [--allow-write] [--allow-command] [--max-steps n]\n  jeden sessions [limit]\n  jeden show <session-id-or-path>\n  jeden export <session-id-or-path> [output.json]\n  jeden artifacts <session-id-or-path>\n  jeden artifact <session-id-or-path> <name> [output]\n  jeden config [--cwd path]\n  jeden doctor [--cwd path]\n  jeden capabilities [--cwd path]\n\nSlash commands:\n  /login [provider]      automated OAuth login from product provider registry\n  /logout <provider>     remove provider auth record\n  /settings              show auth settings\n  /help                  show slash command list\n"
}

fn now_iso() -> String {
    let secs = SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default().as_secs();
    format!("{}", secs)
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
            other if other.starts_with("--") && matches!(args.command.as_str(), "export") => args.positionals.push(other.to_string()),
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

fn write_json<T: Serialize>(path: &Path, value: &T) -> Result<(), String> {
    if let Some(parent) = path.parent() { fs::create_dir_all(parent).map_err(|e| e.to_string())?; }
    let text = serde_json::to_string_pretty(value).map_err(|e| e.to_string())? + "\n";
    fs::write(path, text).map_err(|e| e.to_string())
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

fn env_key(provider: &str, suffix: &str) -> String { format!("JEDEN_{}_{}", provider.to_ascii_uppercase().replace(|c: char| !c.is_ascii_alphanumeric(), "_"), suffix) }
fn pick(values: &[Option<String>]) -> String { values.iter().flatten().find(|s| !s.trim().is_empty()).cloned().unwrap_or_default() }

#[derive(Debug, Clone)]
struct OAuthPreset { auth_url: String, token_url: String, client_id: String, redirect_uri: String, scope: String, open: bool }

fn oauth_preset(provider: &str, config: &Config) -> OAuthPreset {
    let source = config.auth_providers.as_ref().and_then(|m| m.get(provider)).cloned().unwrap_or_default();
    OAuthPreset {
        auth_url: pick(&[source.auth_url, source.authorize_url, env::var(env_key(provider, "AUTH_URL")).ok(), env::var(env_key(provider, "AUTHORIZE_URL")).ok(), if provider == "wisent" { env::var("WISENT_AUTH_URL").ok() } else { None }, if provider == "wisent" { env::var("WISENT_AUTHORIZE_URL").ok() } else { None }]),
        token_url: pick(&[source.token_url, env::var(env_key(provider, "TOKEN_URL")).ok(), if provider == "wisent" { env::var("WISENT_TOKEN_URL").ok() } else { None }]),
        client_id: pick(&[source.client_id, env::var(env_key(provider, "CLIENT_ID")).ok(), if provider == "wisent" { env::var("WISENT_CLIENT_ID").ok() } else { None }]),
        redirect_uri: pick(&[source.redirect_uri, env::var(env_key(provider, "REDIRECT_URI")).ok(), Some(format!("http://127.0.0.1:37371/oauth/{}", provider))]),
        scope: pick(&[source.scope, env::var(env_key(provider, "SCOPE")).ok()]),
        open: source.open.unwrap_or_else(|| env::var(env_key(provider, "OPEN")).map(|v| v != "false" && v != "0").unwrap_or(true)),
    }
}

fn format_auth_status(cwd: &Path) -> String {
    let auth: AuthFile = read_json(&auth_path(cwd));
    let mut out = vec!["Jeden provider/auth settings".to_string(), format!("Workspace: {}", cwd.display()), format!("Auth file: {}", auth_path(cwd).display())];
    if auth.providers.is_empty() { out.push("Configured providers: none".into()); } else { out.push(format!("Configured providers ({})", auth.providers.len())); }
    for (name, record) in auth.providers { out.push(format!("- {}{}", name, if record.active { " (active)" } else { "" })); out.push(format!("  method: {}", record.method)); out.push(format!("  credentials: {} key(s)", record.credentials.len())); }
    out.push("".into());
    out.push("Actions:".into());
    out.push("  /login                                      start configured Wisent OAuth login".into());
    out.push("  /login <provider>                           start configured OAuth login for a provider".into());
    out.push("  /logout <provider>                           remove provider profile".into());
    out.join("\n")
}

fn random_state() -> String { rand::thread_rng().sample_iter(&Alphanumeric).take(24).map(char::from).collect() }

fn open_url(url: &str) {
    let _ = if cfg!(target_os = "macos") { Command::new("open").arg(url).status() } else if cfg!(target_os = "windows") { Command::new("cmd").args(["/C", "start", url]).status() } else { Command::new("xdg-open").arg(url).status() };
}

fn wait_for_callback(redirect_uri: &str, state: &str) -> Result<(String, String), String> {
    let url = Url::parse(redirect_uri).map_err(|e| e.to_string())?;
    let host = url.host_str().unwrap_or("127.0.0.1");
    let port = url.port().unwrap_or(80);
    let listener = TcpListener::bind((host, port)).map_err(|e| e.to_string())?;
    listener.set_nonblocking(true).map_err(|e| e.to_string())?;
    let deadline = SystemTime::now() + Duration::from_secs(120);
    let (mut stream, _) = loop {
        match listener.accept() {
            Ok(pair) => break pair,
            Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                if SystemTime::now() > deadline { return Err("OAuth callback timed out".into()); }
                std::thread::sleep(Duration::from_millis(100));
            }
            Err(error) => return Err(error.to_string()),
        }
    };
    let mut buf = [0u8; 4096];
    let n = stream.read(&mut buf).map_err(|e| e.to_string())?;
    let req = String::from_utf8_lossy(&buf[..n]);
    let first = req.lines().next().ok_or("empty callback request")?;
    let path = first.split_whitespace().nth(1).ok_or("bad callback request")?;
    let callback = format!("{}://{}:{}{}", url.scheme(), host, port, path);
    let cb = Url::parse(&callback).map_err(|e| e.to_string())?;
    let code = cb.query_pairs().find(|(k,_)| k == "code").map(|(_,v)| v.to_string()).ok_or("OAuth callback URL has no authorization code")?;
    let got_state = cb.query_pairs().find(|(k,_)| k == "state").map(|(_,v)| v.to_string()).unwrap_or_default();
    let body = if got_state == state { "OAuth login completed. Return to Jeden.\n" } else { "OAuth state mismatch. Return to Jeden.\n" };
    let _ = stream.write_all(format!("HTTP/1.1 200 OK\r\nContent-Type: text/plain\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}", body.len(), body).as_bytes());
    if got_state != state { return Err("OAuth callback state mismatch".into()); }
    Ok((code, callback))
}

fn exchange_code(preset: &OAuthPreset, code: &str) -> Result<BTreeMap<String, Value>, String> {
    let client = reqwest::blocking::Client::builder().timeout(Duration::from_secs(30)).build().map_err(|e| e.to_string())?;
    let mut form = BTreeMap::new();
    form.insert("grant_type", "authorization_code".to_string());
    form.insert("code", code.to_string());
    form.insert("client_id", preset.client_id.clone());
    form.insert("redirect_uri", preset.redirect_uri.clone());
    let res = client.post(&preset.token_url).header("accept", "application/json").form(&form).send().map_err(|e| e.to_string())?;
    let status = res.status();
    let text = res.text().map_err(|e| e.to_string())?;
    if !status.is_success() { return Err(format!("OAuth token exchange failed ({}) {}", status, text)); }
    let value: Value = serde_json::from_str(&text).map_err(|e| e.to_string())?;
    let mut credentials = BTreeMap::new();
    if let Some(v) = value.get("access_token") { credentials.insert("accessToken".into(), v.clone()); }
    if let Some(v) = value.get("refresh_token") { credentials.insert("refreshToken".into(), v.clone()); }
    if let Some(v) = value.get("scope") { credentials.insert("scope".into(), v.clone()); }
    credentials.insert("raw".into(), value);
    Ok(credentials)
}

fn start_login(cwd: &Path, provider: &str) -> Result<String, String> {
    let name = provider_name(provider).ok_or("invalid provider")?;
    let config = load_config(cwd);
    let preset = oauth_preset(&name, &config);
    let missing = [if preset.auth_url.is_empty() { Some("authorize URL") } else { None }, if preset.token_url.is_empty() { Some("token URL") } else { None }, if preset.client_id.is_empty() { Some("client id") } else { None }].into_iter().flatten().collect::<Vec<_>>();
    if !missing.is_empty() { return Err(format!("Automated login is not configured for {}. Product OAuth preset is incomplete: {}. This is a product configuration error; /login stops here until the product OAuth preset is present.", name, missing.join(", "))); }
    let state = random_state();
    let mut auth_url = Url::parse(&preset.auth_url).map_err(|e| e.to_string())?;
    auth_url.query_pairs_mut().append_pair("response_type", "code").append_pair("client_id", &preset.client_id).append_pair("redirect_uri", &preset.redirect_uri).append_pair("state", &state);
    if !preset.scope.is_empty() { auth_url.query_pairs_mut().append_pair("scope", &preset.scope); }
    let mut auth: AuthFile = read_json(&auth_path(cwd));
    let mut oauth = BTreeMap::new();
    oauth.insert("authUrl".into(), Value::String(preset.auth_url.clone()));
    oauth.insert("tokenUrl".into(), Value::String(preset.token_url.clone()));
    oauth.insert("clientId".into(), Value::String(preset.client_id.clone()));
    oauth.insert("redirectUri".into(), Value::String(preset.redirect_uri.clone()));
    oauth.insert("state".into(), Value::String(state.clone()));
    auth.providers.insert(name.clone(), ProviderRecord { active: false, method: "oauth-pending".into(), updated_at: now_iso(), oauth, credentials: BTreeMap::new(), profile: BTreeMap::new() });
    auth.active_provider = Some(name.clone());
    write_json(&auth_path(cwd), &auth)?;
    if preset.open { open_url(auth_url.as_str()); }
    let (code, callback_url) = wait_for_callback(&preset.redirect_uri, &state)?;
    let credentials = exchange_code(&preset, &code)?;
    let mut auth: AuthFile = read_json(&auth_path(cwd));
    let mut profile = BTreeMap::new();
    profile.insert("callbackUrl".into(), Value::String(callback_url));
    profile.insert("exchangedAt".into(), Value::String(now_iso()));
    let mut oauth = auth.providers.get(&name).map(|r| r.oauth.clone()).unwrap_or_default();
    oauth.insert("scope".into(), Value::String(preset.scope));
    auth.providers.insert(name.clone(), ProviderRecord { active: true, method: "oauth-token".into(), updated_at: now_iso(), oauth, credentials, profile });
    auth.active_provider = Some(name.clone());
    write_json(&auth_path(cwd), &auth)?;
    Ok(format!("OAuth login completed for {} in {}.", name, auth_path(cwd).display()))
}

fn logout(cwd: &Path, provider: &str) -> Result<String, String> {
    let name = provider_name(provider).ok_or("Usage: /logout <provider>")?;
    let mut auth: AuthFile = read_json(&auth_path(cwd));
    if auth.providers.remove(&name).is_none() { return Err(format!("Provider profile not found: {}", name)); }
    if auth.active_provider.as_deref() == Some(&name) { auth.active_provider = None; }
    write_json(&auth_path(cwd), &auth)?;
    Ok(format!("Removed provider profile {} from {}.", name, auth_path(cwd).display()))
}


const SLASH_COMMANDS: &[(&str, &str)] = &[
    ("settings", "Open settings menu"), ("setup", "Open provider setup"), ("plan", "Toggle plan mode"),
    ("goal", "Toggle goal mode"), ("loop", "Toggle loop mode"), ("model", "Switch model"),
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
    ("update", "Show update steps"), ("force", "Force next tool"), ("exit", "Exit"), ("quit", "Quit"),
];

fn format_slash_help() -> String {
    let mut out = String::from("Jeden slash commands:
");
    for (name, description) in SLASH_COMMANDS { out.push_str(&format!("/{:<15} {}
", name, description)); }
    out
}

fn update_text() -> String {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let head = Command::new("git")
        .args(["rev-parse", "--short", "HEAD"])
        .current_dir(&root)
        .output()
        .ok()
        .and_then(|output| if output.status.success() { Some(String::from_utf8_lossy(&output.stdout).trim().to_string()) } else { None })
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| "unknown".into());
    format!(
        "Jeden update\nCurrent source: {}\nCurrent git HEAD: {}\n\nTo update this source install:\n  cd {}\n  git pull --ff-only\n  cargo build --release\n\nIf your shell cannot find the linked bin after updating, run:\n  npm link\n  rehash\n",
        root.display(),
        head,
        root.display()
    )
}

pub(crate) fn handle_slash(cwd: &Path, input: &str, model: Option<&str>) -> Result<String, String> {
    let trimmed = input.trim();
    let session_root = session_root();
    let slash_context = slash::SlashContext { cwd, model, session_root: &session_root };
    if let Some(result) = slash::handle_local(&slash_context, trimmed) {
        return result;
    }
    let mut parts = trimmed.split_whitespace();
    let command = parts.next().unwrap_or("");
    match command {
        "/help" | "/commands" => Ok(format_slash_help()),
        "/settings" | "/setup" | "/providers" => Ok(format_auth_status(cwd)),
        "/login" => start_login(cwd, parts.next().unwrap_or("wisent")),
        "/logout" => logout(cwd, parts.next().unwrap_or("")),
        "/usage" => Ok(crate::slash::handle_local(&slash_context, trimmed).transpose()?.unwrap_or_else(|| "Usage accounting is available in Rust mode-state.".into())),
        "/update" => Ok(update_text()),
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


fn interactive(args: &Args) -> Result<String, String> {
    let config = load_config(&args.cwd);
    let model = args
        .model
        .clone()
        .or(config.model)
        .or_else(|| env::var("JEDEN_MODEL").ok())
        .or_else(|| env::var("MODEL").ok())
        .unwrap_or_else(|| "default".into());
    let mut session_model = Some(model.clone());
    tui::run_basic_loop(
        tui::InteractiveConfig {
            cwd: args.cwd.display().to_string(),
            write_status: if args.allow_write { "allow".into() } else { "ask".into() },
            command_status: if args.allow_command { "allow".into() } else { "ask".into() },
            model,
        },
        |input| {
            if input.trim_start().starts_with('/') {
                let trimmed = input.trim();
                let (command, rest) = trimmed.split_once(char::is_whitespace).unwrap_or((trimmed, ""));
                if matches!(command, "/model" | "/models" | "/switch") {
                    let next = rest.trim();
                    if next.is_empty() {
                        return Ok(format!("Current model route: {}.", session_model.as_deref().unwrap_or("default")));
                    }
                    session_model = Some(next.to_string());
                    return Ok(format!("Model route set to {}.", next));
                }
                if command == "/retry" {
                    let mut run_args = args.clone();
                    run_args.command = "run".into();
                    run_args.model = session_model.clone();
                    run_args.positionals = vec![trimmed.to_string()];
                    run_args.json = false;
                    return agent::retry_command(&run_args).map(|text| text.trim().to_string());
                }
                if command == "/btw" {
                    let mut run_args = args.clone();
                    run_args.command = "run".into();
                    run_args.model = session_model.clone();
                    run_args.positionals = vec![trimmed.to_string()];
                    run_args.json = false;
                    return agent::btw_command(&run_args, rest).map(|text| text.trim().to_string());
                }
                handle_slash(&args.cwd, input, session_model.as_deref())
            } else {
                let mut run_args = args.clone();
                run_args.command = "run".into();
                run_args.model = session_model.clone();
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
        "update" => Ok(update_text()),
        "config" => Ok(serde_json::to_string_pretty(&load_config(&args.cwd)).unwrap() + "\n"),
        "doctor" | "capabilities" => Ok(doctor(&args)),
        other => Err(format!("unknown command: {}", other)),
    };
    match result { Ok(text) => print!("{}", text), Err(e) => { eprintln!("Error: {}", e); std::process::exit(1); } }
}
