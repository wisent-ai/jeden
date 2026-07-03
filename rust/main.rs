use hmac::{Hmac, Mac};
use rand::{distributions::Alphanumeric, Rng};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use std::env;
use std::fs;
use std::io::{Read, Write};
use std::net::TcpListener;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use url::Url;

type HmacSha256 = Hmac<Sha256>;

#[derive(Debug, Clone)]
struct Args {
    raw: Vec<String>,
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
    let raw = argv.clone();
    let mut rest = argv.into_iter();
    let first = rest.next();
    let mut command = first.unwrap_or_else(|| "interactive".to_string());
    if command == "--help" || command == "-h" { return Ok(Args { raw, command: "help".into(), cwd: env::current_dir().unwrap(), model: None, max_tokens: 2048, max_steps: 8, allow_write: false, allow_command: false, json: false, positionals: vec![] }); }
    if command.starts_with("--") { rest = std::iter::once(command).chain(rest).collect::<Vec<_>>().into_iter(); command = "interactive".into(); }
    let mut args = Args { raw, command, cwd: env::current_dir().map_err(|e| e.to_string())?, model: None, max_tokens: 2048, max_steps: 8, allow_write: false, allow_command: false, json: false, positionals: vec![] };
    while let Some(arg) = rest.next() {
        match arg.as_str() {
            "--cwd" => args.cwd = PathBuf::from(rest.next().ok_or("--cwd requires a value")?),
            "--model" => args.model = Some(rest.next().ok_or("--model requires a value")?),
            "--max-tokens" => args.max_tokens = rest.next().ok_or("--max-tokens requires a value")?.parse().map_err(|_| "--max-tokens must be an integer")?,
            "--max-steps" => args.max_steps = rest.next().ok_or("--max-steps requires a value")?.parse().map_err(|_| "--max-steps must be an integer")?,
            "--allow-write" => args.allow_write = true,
            "--allow-command" => args.allow_command = true,
            "--json" => args.json = true,
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

fn slash(cwd: &Path, input: &str) -> Result<String, String> {
    let trimmed = input.trim();
    let mut parts = trimmed.split_whitespace();
    let command = parts.next().unwrap_or("");
    match command {
        "/help" => Ok("Jeden slash commands:\n/login [provider]\n/logout <provider>\n/settings\n/help".into()),
        "/settings" | "/setup" => Ok(format_auth_status(cwd)),
        "/login" => start_login(cwd, parts.next().unwrap_or("wisent")),
        "/logout" => logout(cwd, parts.next().unwrap_or("")),
        _ => Err(format!("Unknown slash command: {}", command)),
    }
}

fn list_sessions(limit: usize) -> String {
    let mut rows = vec![];
    if let Ok(entries) = fs::read_dir(session_root()) {
        for entry in entries.flatten().take(limit) { rows.push(entry.file_name().to_string_lossy().to_string()); }
    }
    if rows.is_empty() { "No sessions found.\n".into() } else { rows.join("\n") + "\n" }
}

fn model_router_config(config: &Config, args: &Args) -> (String, String, String, String) {
    let url = env::var("MODEL_ROUTER_URL").ok().or(config.model_router_url.clone()).unwrap_or_else(|| "https://model-router-1080673333190.us-central1.run.app".into());
    let agent_id = env::var("WISENT_APP_AGENT_ID").ok().or(config.agent_id.clone()).unwrap_or_else(|| "wisent-app".into());
    let secret = env::var("WISENT_APP_AGENT_AUTH_SECRET").unwrap_or_default();
    let model = args.model.clone().or(config.model.clone()).or_else(|| env::var("JEDEN_MODEL").ok()).unwrap_or_else(|| "claude-code-subscription".into());
    (url, agent_id, secret, model)
}

fn hmac_headers(body: &str, agent_id: &str, secret: &str) -> Result<(String, String, String), String> {
    if secret.is_empty() { return Err("WISENT_APP_AGENT_AUTH_SECRET is required".into()); }
    let ts = SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default().as_secs().to_string();
    let body_hash = hex::encode(Sha256::digest(body.as_bytes()));
    let mut mac = HmacSha256::new_from_slice(secret.as_bytes()).map_err(|e| e.to_string())?;
    mac.update(format!("{}:{}:{}", agent_id, ts, body_hash).as_bytes());
    Ok((ts, body_hash, hex::encode(mac.finalize().into_bytes())))
}

fn run_prompt(args: &Args) -> Result<String, String> {
    let task = args.positionals.join(" ");
    if task.trim_start().starts_with('/') { return slash(&args.cwd, task.trim()); }
    let config = load_config(&args.cwd);
    let (url, agent_id, secret, model) = model_router_config(&config, args);
    let body = json!({"model": model, "messages": [{"role":"user", "content": task}], "max_tokens": args.max_tokens}).to_string();
    let (ts, body_hash, sig) = hmac_headers(&body, &agent_id, &secret)?;
    let client = reqwest::blocking::Client::builder().timeout(Duration::from_secs(120)).build().map_err(|e| e.to_string())?;
    let res = client.post(format!("{}/v1/chat/completions", url.trim_end_matches('/'))).header("content-type", "application/json").header("x-agent-id", agent_id).header("x-agent-timestamp", ts).header("x-agent-body-sha256", body_hash).header("x-agent-signature", sig).body(body).send().map_err(|e| e.to_string())?;
    let text = res.text().map_err(|e| e.to_string())?;
    Ok(text)
}

fn doctor(args: &Args) -> String {
    let config = load_config(&args.cwd);
    let (url, agent_id, secret, model) = model_router_config(&config, args);
    json!({"cwd": args.cwd, "modelRouterUrl": url, "agentId": agent_id, "model": model, "secretPresent": !secret.is_empty(), "authFile": auth_path(&args.cwd)}).to_string() + "\n"
}


fn delegate_to_node(args: &[String]) -> ! {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let status = Command::new("node")
        .arg(root.join("src/cli.js"))
        .args(args)
        .status();
    match status {
        Ok(status) => std::process::exit(status.code().unwrap_or(1)),
        Err(error) => {
            eprintln!("Error: failed to delegate to legacy Node CLI: {}", error);
            std::process::exit(1);
        }
    }
}

fn main() {
    let argv = env::args().skip(1).collect::<Vec<_>>();
    let args = match parse_args(argv) { Ok(v) => v, Err(e) => { eprintln!("Error: {}\n{}", e, usage()); std::process::exit(2); } };
    let result = match args.command.as_str() {
        "help" => Ok(usage().to_string()),
        "interactive" => delegate_to_node(&args.raw),
        "run" => {
            let task = args.positionals.join(" ");
            if task.trim_start().starts_with("/login") || task.trim_start().starts_with("/logout") || task.trim_start() == "/settings" {
                run_prompt(&args).map(|s| if args.json { json!({"text": s}).to_string() + "\n" } else { s + "\n" })
            } else {
                delegate_to_node(&args.raw)
            }
        },
        "sessions" => Ok(list_sessions(args.positionals.get(0).and_then(|s| s.parse().ok()).unwrap_or(20))),
        "show" | "export" | "artifacts" | "artifact" | "resume" | "tools" => delegate_to_node(&args.raw),
        "config" => Ok(serde_json::to_string_pretty(&load_config(&args.cwd)).unwrap() + "\n"),
        "doctor" | "capabilities" => Ok(doctor(&args)),
        other => Err(format!("unknown command: {}", other)),
    };
    match result { Ok(text) => print!("{}", text), Err(e) => { eprintln!("Error: {}", e); std::process::exit(1); } }
}
