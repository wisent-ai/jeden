//! Guided environment configuration wizard (`/setup`).
//!
//! The wizard is idempotent: every step checks live state first and renders
//! already-configured items as disabled `[OK]` rows. Nonsecret router settings
//! may be persisted locally; credentials are injected by the trusted Stado
//! launcher and are never written by the wizard.

use std::path::{Path, PathBuf};

use super::common::split_head;
use super::SlashContext;
use crate::cli::config::{load_config, ui_language};
use crate::tui::{CommandOutcome, PickerItem, PickerSpec};

mod env_file;
mod report;

pub(crate) use env_file::brama_router_configured;
use env_file::{configured_value, example_prefill, save_agent_id, save_brama_url};
use report::{checklist_text, validate_text};

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
    SetupState {
        brama_url: configured_value(BRAMA_URL_KEY),
        agent_id: configured_value(AGENT_ID_KEY),
        secret_configured: configured_value(AGENT_SECRET_KEY).is_some(),
        model,
        language: ui_language(&config).code().to_string(),
        theme,
        workspace,
        workspace_error,
    }
}

fn configured_row(label: impl Into<String>, detail: impl Into<String>) -> PickerItem {
    PickerItem::action(label, "")
        .detail(detail)
        .badge("OK")
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
        (None, Some(error)) => items.push(
            PickerItem::action("1. Adopted workspace is unavailable", "")
                .detail(error)
                .badge("ERROR")
                .disabled(true),
        ),
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
    match &state.brama_url {
        Some(url) => items.push(configured_row(
            "2. BRAMA_URL configured",
            format!("{url} · stored in ~/.jeden/.env"),
        )),
        None => {
            let prefill = example_prefill(context.cwd, BRAMA_URL_KEY)
                .map(|url| format!(" {url}"))
                .unwrap_or_else(|| " ".into());
            items.push(
                PickerItem::action("2. Set BRAMA_URL", format!("/setup brama-url{prefill}"))
                    .detail("required Brama model-router endpoint · stored in ~/.jeden/.env")
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
            "injected in memory by the Stado/Skarbiec launcher",
        ));
    } else {
        items.push(configured_row(
            "4. WISENT_APP_AGENT_AUTH_SECRET unavailable",
            "launch with bin/jeden-rust or scripts/run-with-stado.sh",
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

const USAGE: &str =
    "Usage: /setup [status|validate|workspace <path>|brama-url <url>|agent-id <id>|model|preferences]";

/// Text-mode handler (non-interactive callers such as `jeden run "/setup …"`
/// and the slash fallback). Bare `/setup` and `/setup status` print the
/// checklist without changing anything.
pub(crate) fn handle_text(args: &str, context: &SlashContext<'_>) -> Result<String, String> {
    let (verb, rest) = split_head(args);
    match verb.to_ascii_lowercase().as_str() {
        "" | "status" => Ok(checklist_text(context)),
        "validate" => Ok(validate_text(context)),
        "workspace" => {
            let path = if rest.trim().is_empty() {
                context.cwd
            } else {
                Path::new(rest.trim())
            };
            Ok(crate::cli::workspace::adopt(path, context.cwd)?.text())
        }
        "brama-url" => save_brama_url(rest),
        "agent-id" => save_agent_id(rest),
        "model" | "preferences" => Ok(format!(
            "That step needs the interactive TUI; run bare `jeden` and then /setup.\n\n{}",
            checklist_text(context)
        )),
        _ => Err(USAGE.into()),
    }
}

/// Interactive handler for the TUI: step submissions keep the wizard sequence
/// alive by returning the next view instead of plain text.
pub(crate) fn run_interactive(
    cwd: &Path,
    args: &str,
    current_model: Option<&str>,
    interactive: bool,
) -> Result<CommandOutcome, String> {
    let session_root = crate::session_root();
    let context = SlashContext {
        cwd,
        model: current_model,
        session_root: &session_root,
    };
    let (verb, rest) = split_head(args);
    match verb.to_ascii_lowercase().as_str() {
        "" if interactive => setup_picker(&context).map(CommandOutcome::Picker),
        "" | "status" => Ok(CommandOutcome::Text(checklist_text(&context))),
        "validate" => Ok(CommandOutcome::Text(validate_text(&context))),
        "workspace" | "brama-url" | "agent-id" => {
            let message = match verb.to_ascii_lowercase().as_str() {
                "workspace" => {
                    let path = if rest.trim().is_empty() { cwd } else { Path::new(rest.trim()) };
                    crate::cli::workspace::adopt(path, cwd)?.text()
                }
                "brama-url" => save_brama_url(rest)?,
                _ => save_agent_id(rest)?,
            };
            if interactive {
                // Re-open the wizard so the user lands on the next missing step.
                setup_picker(&context).map(CommandOutcome::Picker)
            } else {
                Ok(CommandOutcome::Text(message))
            }
        }
        "model" => crate::cli::run::slash_ui::model_picker(cwd, current_model, false)
            .map(CommandOutcome::Picker)
            .map_err(|error| {
                format!(
                    "{error}\nHint: fix the Brama connection (step 1 writes ~/.jeden/.env), then rerun /setup. Completed steps are kept."
                )
            }),
        "preferences" => Ok(CommandOutcome::Picker(preferences_picker(cwd))),
        _ => Err(USAGE.into()),
    }
}
