use std::path::Path;

use crate::cli::auth::{format_auth_status, provider_picker};
use crate::cli::config::{load_config, schema::settings_picker};
use crate::control_plane::brama::{ModelEntry, ModelPerf, ModelPrice};
use crate::slash::{self, SlashContext};
use crate::tui::{CommandOutcome, PickerItem, PickerSpec};

/// Compact price per million tokens: up to 2 decimals, trailing zeros dropped
/// ($5/30, $0.25/1.25, $2.5/15).
fn format_price(value: f64) -> String {
    let rounded = (value * 100.0).round() / 100.0;
    if rounded.fract() == 0.0 {
        format!("{}", rounded as i64)
    } else {
        format!("{rounded:.2}")
            .trim_end_matches('0')
            .trim_end_matches('.')
            .to_string()
    }
}

fn price_detail(price: &ModelPrice) -> String {
    if price.input > 0.0 || price.output > 0.0 {
        format!(
            " · ${}/{}",
            format_price(price.input),
            format_price(price.output)
        )
    } else {
        " · free".into()
    }
}

/// Observed performance: latency in seconds (1 decimal) and tokens/sec
/// (rounded), appended only when the router has stats for the route.
fn perf_detail(perf: Option<&ModelPerf>) -> String {
    match perf {
        Some(perf) if perf.count > 0 => {
            format!(" · {:.1}s {:.0}t/s", perf.latency_ms / 1000.0, perf.tps)
        }
        _ => String::new(),
    }
}

/// Provider group: the id segment before the first `/`; ids without `/` group
/// under their literal id.
fn provider_group(id: &str) -> &str {
    id.split_once('/').map(|(head, _)| head).unwrap_or(id)
}

/// Subscription providers first in a fixed order, everything else alphabetical.
fn provider_rank(provider: &str) -> (u8, &str) {
    match provider {
        "claude-code" => (0, ""),
        "codex" => (1, ""),
        "kimi" => (2, ""),
        other => (3, other),
    }
}

/// Active and credential-backed models first; the unavailable tail last.
fn model_rank(model: &ModelEntry, active: Option<&str>) -> u8 {
    if !model.available {
        2
    } else if active == Some(model.id.as_str()) {
        0
    } else {
        1
    }
}

fn model_row(model: &ModelEntry, active: Option<&str>) -> PickerItem {
    let selected = active == Some(model.id.as_str());
    let detail = format!(
        "context {} · output {} · {}{}",
        model.context_window,
        model.max_output_tokens,
        if model.tools { "tools" } else { "no tools" },
        if model.reasoning { " · reasoning" } else { "" }
    );
    let detail = model.unavailable_reason.clone().unwrap_or(detail);
    PickerItem::action(&model.id, format!("/model {}", model.id))
        .detail(format!(
            "{}{}{}",
            detail,
            price_detail(&model.price),
            perf_detail(model.perf.as_ref())
        ))
        .badge(if !model.available {
            "UNAVAILABLE"
        } else if selected {
            "ACTIVE"
        } else {
            "AVAILABLE"
        })
        .disabled(selected || !model.available)
}

/// Non-action group header; the picker skips disabled, command-less rows.
fn group_header(provider: &str, count: usize) -> PickerItem {
    let mut header = PickerItem::action(format!("── {provider} ({count}) ──"), "");
    header.command = None;
    header.disabled = true;
    header
}

pub(crate) fn model_picker(
    cwd: &Path,
    current_model: Option<&str>,
    show_all: bool,
) -> Result<PickerSpec, String> {
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
    let total = catalog.models.len();
    let active = active.as_deref();
    // Curated mode lists the active route plus credential-backed models; when
    // neither exists there is nothing to curate, so show the full catalog.
    let show_all = show_all
        || !catalog
            .models
            .iter()
            .any(|model| model.available || active == Some(model.id.as_str()));
    let mut items = Vec::new();
    for (route, detail) in [
        ("any", "auto-select across available subscriptions"),
        ("any-vision-capable", "auto-select a vision-capable subscription"),
    ] {
        let selected = active == Some(route);
        items.push(
            PickerItem::action(route, format!("/model {route}"))
                .detail(detail)
                .badge(if selected { "ACTIVE" } else { "AUTO" })
                .disabled(selected),
        );
    }
    let mut models = catalog.models;
    if show_all {
        models.sort_by(|left, right| {
            provider_rank(provider_group(&left.id))
                .cmp(&provider_rank(provider_group(&right.id)))
                .then_with(|| model_rank(left, active).cmp(&model_rank(right, active)))
                .then_with(|| left.id.cmp(&right.id))
        });
        let mut group_counts = std::collections::HashMap::new();
        for model in &models {
            *group_counts
                .entry(provider_group(&model.id))
                .or_insert(0_usize) += 1;
        }
        let mut current_group: Option<&str> = None;
        for model in &models {
            let provider = provider_group(&model.id);
            if current_group != Some(provider) {
                let count = group_counts.get(provider).copied().unwrap_or_default();
                items.push(group_header(provider, count));
                current_group = Some(provider);
            }
            items.push(model_row(model, active));
        }
        items.push(PickerItem::action("Show configured only", "/model").badge("MORE"));
    } else {
        models.retain(|model| model.available || active == Some(model.id.as_str()));
        models.sort_by(|left, right| {
            model_rank(left, active)
                .cmp(&model_rank(right, active))
                .then_with(|| left.id.cmp(&right.id))
        });
        items.extend(models.iter().map(|model| model_row(model, active)));
        items.push(
            PickerItem::action(format!("Show all {total} models"), "/model --all").badge("MORE"),
        );
    }
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
    if matches!(command, "/model" | "/models" | "/switch") && matches!(args.trim(), "--all" | "-a")
    {
        return Some(model_picker(cwd, model, true).map(CommandOutcome::Picker));
    }
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
            return Some(model_picker(cwd, model, false).map(CommandOutcome::Picker))
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
