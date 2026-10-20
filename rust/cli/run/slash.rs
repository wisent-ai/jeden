//! Slash help, builtin routing, model/settings slash, and self-update.

use serde_json::Value;
use std::env;
use std::path::{Path, PathBuf};

use crate::cli::auth::{format_auth_status, logout, refresh, start_login};
use crate::cli::config::load_config;
use crate::cli::config::schema::config_command;
use crate::{config_path, session_root, slash, Args};

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
    // The documented default channel applies only when none is declared; a
    // declaration that is not text is refused, not read as the default.
    let channel = match env::var("JEDEN_UPDATE_CHANNEL") {
        Ok(channel) => channel,
        Err(env::VarError::NotPresent) => "stable".to_string(),
        Err(error) => return Err(format!("JEDEN_UPDATE_CHANNEL cannot be read: {error}")),
    };
    // The native triple is asked only when none is declared: asked first, a
    // host whose own triple is unknown refused even a declared target.
    let target_triple = match env::var("JEDEN_UPDATE_TARGET_TRIPLE") {
        Ok(triple) => triple,
        Err(env::VarError::NotPresent) => super::update::native_target_triple()?,
        Err(error) => {
            return Err(format!(
                "JEDEN_UPDATE_TARGET_TRIPLE cannot be read: {error}"
            ))
        }
    };
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

/// Resolve a model route against the selected model catalog, returning its
/// canonical id. A bare id resolves to a unique provider route; ambiguity
/// names every matching route.
pub(crate) fn resolve_model_route(cwd: &Path, model: &str) -> Result<String, String> {
    if crate::model_router::is_virtual_model_route(model) {
        if crate::agent::credential::direct_provider().is_some() {
            return Err(format!(
                "{model} is a Brama virtual route; choose a concrete direct-provider model with /model"
            ));
        }
        if let Some(cause) = crate::agent::credential::direct_refusal() {
            return Err(cause.to_owned());
        }
        return Ok(model.to_string());
    }
    let client = crate::control_plane::model_client()?;
    let catalog = crate::control_plane::model_catalog(cwd, &client, false)
        .map_err(|error| error.to_string())?;
    match catalog.resolve(model) {
        Ok(entry) => Ok(entry.id.clone()),
        Err(error) => {
            if model.contains('/') {
                return Err(error.to_string());
            }
            match catalog.resolve_bare(model)? {
                Some(entry) => Ok(entry.id.clone()),
                None => Err(error.to_string()),
            }
        }
    }
}

pub(crate) fn set_model_route(cwd: &Path, model: &str) -> Result<String, String> {
    let resolved = resolve_model_route(cwd, model)?;
    let path = config_path(cwd);
    let mut config = crate::slash::common::project_config_for_edit(cwd)?;
    config
        .as_object_mut()
        .expect("a configuration layer is an object")
        .insert("model".into(), Value::String(resolved.clone()));
    crate::slash::common::write_json_value(&path, &config)?;
    Ok(format!("Model route set to {}.", resolved))
}

fn handle_model_slash(
    cwd: &Path,
    current_model: Option<&str>,
    args: &str,
) -> Result<String, String> {
    let next = args.trim();
    if matches!(next, "--all" | "-a") {
        return super::slash_ui::model_picker(cwd, current_model, true)
            .map(|spec| crate::tui::CommandOutcome::Picker(spec).into_text());
    }
    if next.is_empty() {
        let configured = if let Some(model) = current_model {
            Some(model.to_owned())
        } else if let Some(model) = load_config(cwd).model {
            Some(model)
        } else {
            match env::var("JEDEN_MODEL") {
                Ok(model) => Some(model),
                Err(env::VarError::NotPresent) => None,
                Err(error) => return Err(format!("JEDEN_MODEL cannot be read: {error}")),
            }
        };
        return Ok(match configured {
            Some(model) => format!("Current model route: {model}."),
            None if crate::agent::credential::direct_provider().is_some() => {
                "No model route selected; choose one advertised by the direct provider.".into()
            }
            None => "No model route selected; choose one advertised by Brama.".into(),
        });
    }
    set_model_route(cwd, next)
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
    // The verb is `jeden config`'s: it refuses an unknown one with its usage,
    // so `/settings` and the CLI accept and refuse the same verbs.
    if positionals.is_empty() {
        return Ok(format_auth_status(cwd));
    }
    // max_tokens/max_steps are irrelevant to config_command; Default fills them
    // number-free.
    config_command(&Args {
        command: crate::Command::Config,
        cwd: cwd.to_path_buf(),
        json,
        positionals,
        ..Default::default()
    })
}

/// Usage text for a bare (or unrecognised) billing-family command, derived from
/// the routing table so it cannot drift from what actually dispatches.
fn billing_slash_usage(command: &str) -> String {
    let mut usage = format!("Usage: {command} <subcommand>");
    for (prefix, _) in crate::cli::billing::billing_slash_handlers() {
        if prefix
            .strip_prefix(command)
            .is_some_and(|rest| rest.starts_with(' '))
        {
            usage.push('\n');
            usage.push_str(prefix);
        }
    }
    usage
}

pub(crate) fn handle_slash(cwd: &Path, input: &str, model: Option<&str>) -> Result<String, String> {
    let trimmed = input.trim();
    let mut parts = trimmed.split_whitespace();
    let Some(command) = parts.next() else {
        return Err("slash command is empty".into());
    };
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
        "/help" => {
            let help = format_slash_help(cwd);
            Ok(help)
        }
        "/settings" => handle_settings_slash(cwd, parts.collect::<Vec<_>>().join(" ").as_str()),
        "/providers" => Ok(format_auth_status(cwd)),
        "/login" => start_login(cwd, parts.collect::<Vec<_>>().join(" ").as_str()),
        "/token" => crate::cli::token::token_slash(),
        "/logout" => logout(cwd, parts.collect::<Vec<_>>().join(" ").as_str()),
        "/refresh" => refresh(parts.collect::<Vec<_>>().join(" ").as_str()),
        "/model" => handle_model_slash(cwd, model, parts.collect::<Vec<_>>().join(" ").as_str()),
        "/exit" => Ok("Exit is handled by the interactive input loop.".into()),
        "/billing" | "/subscriptions" | "/payment-method" => Ok(billing_slash_usage(command)),
        _ => Err(format!("Unknown Rust slash command: {}", command)),
    }
}
