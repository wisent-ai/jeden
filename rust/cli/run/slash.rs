//! Slash help, builtin routing, model/settings slash, and self-update.

use serde_json::{json, Value};
use std::env;
use std::fs;
use std::path::{Path, PathBuf};

use crate::cli::auth::{format_auth_status, logout, refresh, start_login};
use crate::cli::config::load_config;
use crate::cli::config::schema::config_command;
use crate::{config_path, read_json, session_root, slash, Args};

fn format_slash_help(cwd: &Path) -> String {
    let mut out = String::from("Jeden slash commands:\n");
    for descriptor in crate::capability::slash_descriptors(cwd) {
        let Some(command) = descriptor.ui.action.as_deref() else {
            continue;
        };
        out.push_str(&format!(
            "/{:<15} {}\n",
            command.trim_start_matches('/'),
            descriptor.ui.description
        ));
    }
    out
}

/// True for every slash command Jeden handles itself (canonical list + aliases).
/// Unknown slash input forwards to the model as a prompt instead of hard-erroring.
pub(crate) fn is_builtin_slash(command: &str) -> bool {
    crate::capability::is_builtin_slash(command)
}

pub(crate) fn update_command() -> Result<String, String> {
    let manifest_location = env::var("JEDEN_UPDATE_MANIFEST").map_err(|_| {
        "JEDEN_UPDATE_MANIFEST must point to an HTTPS or local DSSE release manifest".to_string()
    })?;
    let channel = env::var("JEDEN_UPDATE_CHANNEL").unwrap_or_else(|_| "stable".into());
    let target_triple =
        env::var("JEDEN_UPDATE_TARGET_TRIPLE").unwrap_or(super::update::native_target_triple()?);
    let target = match env::var_os("JEDEN_UPDATE_TARGET") {
        Some(path) => PathBuf::from(path),
        None => std::env::current_exe().map_err(|error| error.to_string())?,
    };
    let manifest = super::update::execute(super::update::UpdateRequest {
        manifest_location,
        channel,
        target_triple,
        target,
        roots: super::update::embedded_trust_roots()?,
        current_version: semver::Version::parse(crate::JEDEN_VERSION)
            .map_err(|error| error.to_string())?,
        now: None,
        failpoint: None,
    })?;
    Ok(format!(
        "Jeden update {} ({}) installed and post-health verified\n",
        manifest.version, manifest.sha256
    ))
}

pub(crate) fn resolve_model_route(cwd: &Path, model: &str) -> Result<(), String> {
    let runtime_config = load_config(cwd);
    let endpoint = env::var("BRAMA_URL")
        .ok()
        .or(runtime_config.model_router_url);
    let client = crate::control_plane::brama::BramaClient::configured(
        endpoint,
        env::var("BRAMA_TOKEN").ok(),
    );
    crate::control_plane::model_catalog(cwd, &client, false)
        .and_then(|catalog| catalog.resolve(model).map(|_| ()))
        .map_err(|error| error.to_string())
}

fn handle_model_slash(
    cwd: &Path,
    current_model: Option<&str>,
    args: &str,
) -> Result<String, String> {
    let next = args.trim();
    if next.is_empty() {
        let configured = current_model
            .map(str::to_string)
            .or_else(|| load_config(cwd).model)
            .or_else(|| env::var("JEDEN_MODEL").ok());
        return Ok(configured
            .map(|model| format!("Current model route: {model}."))
            .unwrap_or_else(|| "No model route selected; choose one advertised by Brama.".into()));
    }
    resolve_model_route(cwd, next)?;
    let path = config_path(cwd);
    let mut config = read_json::<Value>(&path);
    if !config.is_object() {
        config = json!({});
    }
    config
        .as_object_mut()
        .expect("object")
        .insert("model".into(), Value::String(next.to_string()));
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|error| error.to_string())?;
    }
    fs::write(
        &path,
        serde_json::to_string_pretty(&config).map_err(|error| error.to_string())? + "\n",
    )
    .map_err(|error| error.to_string())?;
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
        return Err(
            "Usage: /settings [status|list|path|get <key>|set <key> <value>|reset <key>] [--json]"
                .into(),
        );
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

pub(crate) fn handle_slash(cwd: &Path, input: &str, model: Option<&str>) -> Result<String, String> {
    let trimmed = input.trim();
    let mut parts = trimmed.split_whitespace();
    let command = parts.next().unwrap_or("");
    if command.eq_ignore_ascii_case("/update") {
        return update_command();
    }
    let session_root = session_root();
    let slash_context = slash::SlashContext {
        cwd,
        model,
        session_root: &session_root,
    };
    if let Some(result) = slash::handle_local(&slash_context, trimmed) {
        return result;
    }
    if crate::cli::billing::billing_slash_handlers()
        .iter()
        .any(|(prefix, _)| {
            trimmed == *prefix
                || trimmed
                    .strip_prefix(prefix)
                    .is_some_and(|rest| rest.starts_with(' '))
        })
    {
        return crate::cli::billing::handle_billing_slash(trimmed, false);
    }
    match command {
        "/help" | "/commands" => {
            let help = format_slash_help(cwd);
            Ok(help)
        }
        "/settings" => handle_settings_slash(cwd, parts.collect::<Vec<_>>().join(" ").as_str()),
        "/setup" | "/providers" => Ok(format_auth_status(cwd)),
        "/login" => start_login(cwd, parts.collect::<Vec<_>>().join(" ").as_str()),
        "/logout" => logout(cwd, parts.collect::<Vec<_>>().join(" ").as_str()),
        "/refresh" => refresh(parts.collect::<Vec<_>>().join(" ").as_str()),
        "/model" | "/models" | "/switch" => {
            handle_model_slash(cwd, model, parts.collect::<Vec<_>>().join(" ").as_str())
        }
        "/exit" | "/quit" => Ok("Exit is handled by the interactive input loop.".into()),
        _ => Err(format!("Unknown Rust slash command: {}", command)),
    }
}
