use std::path::Path;

use crate::cli::auth::{format_auth_status, provider_picker};
use crate::cli::config::{load_config, schema::settings_picker};
use crate::slash::{self, SlashContext};
use crate::tui::{CommandOutcome, PickerItem, PickerSpec};

fn model_picker(cwd: &Path, current_model: Option<&str>) -> Result<PickerSpec, String> {
    let config = load_config(cwd);
    let endpoint = std::env::var("BRAMA_URL")
        .ok()
        .or(config.model_router_url.clone());
    let active = current_model
        .map(str::to_string)
        .or(config.model)
        .or_else(|| std::env::var("JEDEN_MODEL").ok());
    let client = crate::control_plane::brama::BramaClient::configured(
        endpoint,
        std::env::var("BRAMA_TOKEN").ok(),
    );
    if !client.health().available {
        return Err(client.health().detail);
    }
    let catalog = crate::control_plane::model_catalog(cwd, &client, false)
        .map_err(|error| error.to_string())?;
    let items = catalog
        .models
        .into_iter()
        .map(|model| {
            let selected = active.as_deref() == Some(model.id.as_str());
            let detail = format!(
                "context {} · output {} · {}{}",
                model.context_window,
                model.max_output_tokens,
                if model.tools { "tools" } else { "no tools" },
                if model.reasoning { " · reasoning" } else { "" }
            );
            PickerItem::action(&model.id, format!("/model {}", model.id))
                .detail(model.unavailable_reason.unwrap_or(detail))
                .badge(if !model.available {
                    "UNAVAILABLE"
                } else if selected {
                    "ACTIVE"
                } else {
                    "AVAILABLE"
                })
                .disabled(selected || !model.available)
        })
        .collect();
    Ok(PickerSpec::new("Select model route", items))
}

fn logout_picker() -> Result<PickerSpec, String> {
    let client = crate::control_plane::weles::WelesClient::from_env();
    if !client.health().available {
        return Err(client.health().detail);
    }
    let items = client
        .accounts(None)
        .map_err(|error| error.to_string())?
        .into_iter()
        .map(|account| {
            PickerItem::action(&account.display_name, format!("/logout {}", account.id))
                .detail(format!("{} · {}", account.provider, account.status))
                .badge("ACCOUNT")
        })
        .collect();
    Ok(PickerSpec::new("Select account to logout", items))
}

pub(crate) fn interactive_view(
    cwd: &Path,
    input: &str,
    model: Option<&str>,
) -> Option<Result<CommandOutcome, String>> {
    let _capabilities = crate::capability::for_cwd(cwd);
    let trimmed = input.trim();
    let (command, args) = trimmed
        .split_once(char::is_whitespace)
        .unwrap_or((trimmed, ""));
    if !args.trim().is_empty() {
        return None;
    }
    if let Some(view) = crate::capability::view_descriptor(cwd, command) {
        if !view.health.is_executable() || !view.ui.executable {
            let detail = view
                .health
                .detail
                .unwrap_or_else(|| "Capability backend unavailable".into());
            return Some(Ok(CommandOutcome::Picker(PickerSpec::new(
                format!("{} unavailable", view.ui.label),
                vec![PickerItem::action(view.ui.label, "")
                    .detail(detail)
                    .badge("UNAVAILABLE")
                    .disabled(true)],
            ))));
        }
    }
    match command {
        "/login" => {
            return Some(Ok(CommandOutcome::Text(format_auth_status(cwd))));
        }
        "/setup" | "/providers" => {
            return Some(provider_picker(cwd).map(CommandOutcome::Picker));
        }
        "/logout" => return Some(logout_picker().map(CommandOutcome::Picker)),
        "/model" | "/models" | "/switch" => {
            return Some(model_picker(cwd, model).map(CommandOutcome::Picker))
        }
        "/settings" => return Some(Ok(CommandOutcome::Picker(settings_picker(cwd)))),
        _ => {}
    }
    let session_root = crate::session_root();
    let context = SlashContext {
        cwd,
        model,
        session_root: &session_root,
    };
    slash::interactive_picker(&context, input).map(|picker| picker.map(CommandOutcome::Picker))
}
