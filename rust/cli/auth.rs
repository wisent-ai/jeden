//! Auth-broker integration and provider/login/logout helpers.

use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeMap;
use std::io::IsTerminal;
use std::path::Path;
use std::process::Command;
use std::env;

use crate::{auth_path, read_json};

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub(crate) struct AuthProviderConfig {
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

fn provider_name(value: &str) -> Option<String> {
    let name = value.trim().to_ascii_lowercase();
    let mut chars = name.chars();
    let first_ok = matches!(chars.next(), Some(c) if c.is_ascii_lowercase() || c.is_ascii_digit());
    if !first_ok { return None; }
    if chars.all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || matches!(c, '.' | '_' | '-')) { Some(name) } else { None }
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
            // The prior display cap was an unconsented numeric literal; list every provider.
            let names = providers.iter().filter_map(|provider| {
                let id = provider.get("id").and_then(Value::as_str)?;
                let name = provider.get("name").and_then(Value::as_str).unwrap_or(id);
                Some(format!("{} ({})", id, name))
            }).collect::<Vec<_>>();
            format!("OMP auth-broker providers: {}.", names.join(", "))
        }
        Err(error) => format!("OMP auth-broker providers: unavailable ({})", error.lines().next().unwrap_or("unknown error")),
    }
}

pub(crate) fn format_auth_status(cwd: &Path) -> String {
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

pub(crate) fn start_login(_cwd: &Path, args: &str) -> Result<String, String> {
    let tokens = args.split_whitespace().collect::<Vec<_>>();
    let provider_tokens = tokens.iter().copied().filter(|token| !token.starts_with("--")).collect::<Vec<_>>();
    let candidate = match provider_tokens.as_slice() {
        [] => {
            let broker_args = parse_broker_args(args, true)?;
            return auth_broker_command("login", &broker_args);
        }
        [only] => *only,
        _ => return Ok("Warning: No OAuth login is waiting for a manual callback.".into()),
    };
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

pub(crate) fn logout(_cwd: &Path, args: &str) -> Result<String, String> {
    let broker_args = parse_broker_args(args, true)?;
    auth_broker_command("logout", &broker_args)
}
