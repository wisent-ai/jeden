//! Guided environment configuration wizard (`/setup`).
//!
//! The wizard is idempotent: every step checks live state first and renders
//! already-configured items as disabled `[OK]` rows. Nonsecret router settings
//! may be persisted locally; credentials are injected by the trusted Stado
//! launcher and are never written by the wizard.

use std::path::{Path, PathBuf};

use super::SlashContext;
use crate::cli::config::{load_config, ui_language};
use crate::tui::{PickerItem, PickerSpec};

mod env_file;
mod report;
mod steps;

pub(crate) use env_file::brama_router_configured;
use env_file::{configured_value, example_prefill};
pub(crate) use steps::{handle_text, run_interactive};

const BRAMA_URL_KEY: &str = "BRAMA_URL";
const AGENT_ID_KEY: &str = "WISENT_APP_AGENT_ID";
const AGENT_SECRET_KEY: &str = "WISENT_APP_AGENT_AUTH_SECRET";
const DEFAULT_AGENT_ID: &str = "wisent-app";

struct SetupState {
    brama_url: Option<String>,
    agent_id: Option<String>,
    secret_configured: bool,
    workspace: Option<PathBuf>,
    workspace_error: Option<String>,
    /// Why `~/.jeden/.env` could not be read; the values it would hold are
    /// then unknown, not unconfigured.
    env_file_error: Option<String>,
    model: Option<String>,
    language: String,
    theme: String,
}

fn setup_state(context: &SlashContext<'_>) -> SetupState {
    let config = load_config(context.cwd);
    let model = context
        .model
        .map(str::to_string)
        .or(config.model.clone())
        .or_else(|| std::env::var("JEDEN_MODEL").ok())
        .filter(|value| !value.trim().is_empty());
    let theme = crate::cli::config::merged_config_value(context.cwd)
        .get("ui")
        .and_then(|ui| ui.get("theme"))
        .and_then(serde_json::Value::as_str)
        .unwrap_or("auto")
        .to_string();
    let (workspace, workspace_error) = match crate::cli::workspace::status(context.cwd) {
        Ok(Some(report)) => (Some(report.workspace), None),
        Ok(None) => (None, None),
        Err(error) => (None, Some(error)),
    };
    // A file that cannot be read leaves its values unknown, and says so once.
    let mut env_file_error = None;
    let mut from_env_file = |key: &str| match configured_value(key) {
        Ok(value) => value,
        Err(error) => {
            env_file_error = Some(error);
            None
        }
    };
    // The route this process resolved, or an address /setup wrote to
    // ~/.jeden/.env after the process resolved it (the next start reads it).
    let brama_url = crate::agent::credential::brama_url().or_else(|| from_env_file(BRAMA_URL_KEY));
    let agent_id = from_env_file(AGENT_ID_KEY);
    let secret_configured = from_env_file(AGENT_SECRET_KEY).is_some();
    SetupState {
        brama_url,
        agent_id,
        secret_configured,
        model,
        language: ui_language(&config).code().to_string(),
        theme,
        workspace,
        workspace_error,
        env_file_error,
    }
}

fn configured_row(label: impl Into<String>, detail: impl Into<String>) -> PickerItem {
    PickerItem::action(label, "")
        .detail(detail)
        .badge("OK")
        .disabled(true)
}

fn error_row(label: impl Into<String>, detail: impl Into<String>) -> PickerItem {
    PickerItem::action(label, "")
        .detail(detail)
        .badge("ERROR")
        .disabled(true)
}

/// Wizard overview picker: one row per step, `[OK]`-disabled when already
/// configured, INPUT/picker dispatch when action is needed.
pub(crate) fn setup_picker(context: &SlashContext<'_>) -> Result<PickerSpec, String> {
    let lang = crate::cli::i18n::lang_code(context.cwd);
    let state = setup_state(context);
    let mut items = Vec::new();
    match (&state.workspace, &state.workspace_error) {
        (Some(workspace), _) => items.push(configured_row(
            "1. Existing workspace adopted",
            format!(
                "{} · future tasks use this path unless --cwd is supplied",
                workspace.display()
            ),
        )),
        (None, Some(error)) => items.push(error_row("1. Adopted workspace is unavailable", error)),
        (None, None) => items.push(
            PickerItem::action(
                "1. Adopt this existing workspace",
                format!("/setup workspace {}", context.cwd.display()),
            )
            .detail("keeps the working tree and session history in place")
            .badge("WORKSPACE")
            .prefill(),
        ),
    }
    if let Some(error) = &state.env_file_error {
        items.push(error_row("Environment file is unreadable", error));
    }
    match &state.brama_url {
        Some(url) => items.push(configured_row(
            "2. BRAMA_URL configured",
            format!("{url} · {}", crate::agent::credential::route_origin()),
        )),
        None if state.env_file_error.is_some() => items.push(error_row(
            "2. BRAMA_URL state is unknown",
            "Resolve the environment file read error before configuring this value.",
        )),
        None => {
            let prefill = match example_prefill(context.cwd, BRAMA_URL_KEY) {
                Ok(Some(url)) => format!(" {url}"),
                Ok(None) => " ".to_string(),
                Err(error) => {
                    items.push(error_row(".env.example is unreadable", error));
                    " ".to_string()
                }
            };
            items.push(
                PickerItem::action("2. Set BRAMA_URL", format!("/setup brama-url{prefill}"))
                    .detail(format!(
                        "{} · without Stado, the address is stored in ~/.jeden/.env",
                        crate::agent::credential::route_origin()
                    ))
                    .badge("INPUT")
                    .prefill(),
            );
        }
    }
    match &state.agent_id {
        Some(id) => items.push(configured_row(
            "3. WISENT_APP_AGENT_ID configured",
            format!("{id} · stored in ~/.jeden/.env"),
        )),
        None if state.env_file_error.is_some() => items.push(error_row(
            "3. WISENT_APP_AGENT_ID state is unknown",
            "Resolve the environment file read error before configuring this value.",
        )),
        None => items.push(
            PickerItem::action(
                "3. Set WISENT_APP_AGENT_ID",
                format!("/setup agent-id {DEFAULT_AGENT_ID}"),
            )
            .detail(format!(
                "default: {DEFAULT_AGENT_ID} · stored in ~/.jeden/.env"
            ))
            .badge("INPUT")
            .prefill(),
        ),
    }
    if state.secret_configured {
        items.push(configured_row(
            "4. WISENT_APP_AGENT_AUTH_SECRET configured",
            "read by Jeden with `stado credentials get --role wisent-app-agent --field value`",
        ));
    } else if state.env_file_error.is_some() {
        items.push(error_row(
            "4. WISENT_APP_AGENT_AUTH_SECRET state is unknown",
            "Resolve the environment file read error before interpreting this value as unavailable.",
        ));
    } else {
        items.push(configured_row(
            "4. WISENT_APP_AGENT_AUTH_SECRET unavailable",
            "`stado credentials get --role wisent-app-agent --field value` must answer for Stado's identity",
        ));
    }
    match &state.model {
        Some(model) => items.push(configured_row(
            format!("5. Model route: {model}"),
            "change anytime with /model",
        )),
        None => items.push(
            PickerItem::action("5. Select model route", "/setup model")
                .detail("fetch the Brama catalog and pick a route")
                .badge("MODEL"),
        ),
    }
    items.push(
        PickerItem::action("6. Language & theme (optional)", "/setup preferences")
            .detail(format!(
                "current: language {} · theme {}",
                state.language, state.theme
            ))
            .badge("PREFS"),
    );
    items.push(
        PickerItem::action("7. Validate setup", "/setup validate")
            .detail("run doctor and show the final summary")
            .badge("CHECK"),
    );
    Ok(PickerSpec::new("Setup — existing workspace and model access", items).localized(&lang))
}

/// Optional preferences: the existing enum setting rows from the settings
/// view, filtered to `ui.language` and `ui.theme` — selection dispatches the
/// stock `/settings set ...` commands, nothing is rebuilt.
fn preferences_picker(cwd: &Path) -> PickerSpec {
    let lang = crate::cli::i18n::lang_code(cwd);
    let settings = crate::cli::config::schema::settings_picker(cwd);
    let items = settings
        .items
        .into_iter()
        .filter(|item| {
            item.command.as_deref().is_some_and(|command| {
                command.starts_with("/settings set ui.language ")
                    || command.starts_with("/settings set ui.theme ")
            })
        })
        .collect();
    PickerSpec::new("Preferences — language & theme", items).localized(&lang)
}
