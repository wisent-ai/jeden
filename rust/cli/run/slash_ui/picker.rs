//! Building the model picker an operator chooses a route from.
//!
//! Split out of `cli/run/slash_ui.rs`, which had grown past the module line
//! cap.

use super::columns::metric_widths;
use super::rows::{model_rank, model_row, provider_group, provider_rank, summary_key, summary_row};
use crate::cli::config::load_config;
use crate::cli::config::ui_language;
use crate::cli::i18n::tr;
use crate::cli::run::slash_ui::rows::SUBSCRIPTION_PROVIDERS;
use crate::tui::{PickerItem, PickerSpec};
use std::path::Path;

pub(crate) fn model_picker(
    cwd: &Path,
    current_model: Option<&str>,
    show_all: bool,
) -> Result<PickerSpec, String> {
    let config = load_config(cwd);
    let lang = ui_language(&config).code().to_string();
    let endpoint = std::env::var("BRAMA_URL")
        .ok()
        .filter(|value| !value.trim().is_empty());
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
        (
            "any-vision-capable",
            "auto-select a vision-capable subscription",
        ),
    ] {
        let selected = active == Some(route);
        items.push(
            PickerItem::action(route, format!("/model {route}"))
                .detail(detail)
                .badge(if selected {
                    tr(&lang, "badge.active")
                } else {
                    tr(&lang, "badge.auto")
                })
                .disabled(selected),
        );
    }
    let mut models = catalog.models;
    // Summary block: one row per subscription provider with ≥1 available
    // model (●), plus the public-catalog remainder (○). Disabled and
    // command-less, so the picker skips them like group headers.
    let mut summary = Vec::new();
    let mut covered = 0_usize;
    for provider in SUBSCRIPTION_PROVIDERS {
        let count = models
            .iter()
            .filter(|model| model.available && provider_group(&model.id) == *provider)
            .count();
        if count > 0 {
            covered += count;
            summary.push(summary_row(
                format!("● {provider}"),
                tr(
                    &lang,
                    summary_key("picker.summary.subscription", &lang, count),
                )
                .replace("{}", &count.to_string()),
            ));
        }
    }
    let remainder = total.saturating_sub(covered);
    let catalog_row = summary_row(
        "○ catalog".to_string(),
        tr(
            &lang,
            summary_key("picker.summary.catalog", &lang, remainder),
        )
        .replace("{}", &remainder.to_string()),
    );
    if show_all {
        // No summary rows here: with a category bar the picker renders as two
        // panes and the left one already lists every provider with its count.
        // Repeating them among the models would be the same data twice.
        models.sort_by(|left, right| {
            provider_rank(provider_group(&left.id))
                .cmp(&provider_rank(provider_group(&right.id)))
                .then_with(|| model_rank(left, active).cmp(&model_rank(right, active)))
                .then_with(|| left.id.cmp(&right.id))
        });
        // Category bar: one tab per subscription provider present in the
        // catalog, plus a single aggregate "catalog" tab for everything else.
        // Text export re-renders these as `── tab (n) ──` sections.
        let mut tabs = vec![tr(&lang, "picker.tab.all").to_string()];
        for provider in SUBSCRIPTION_PROVIDERS {
            if models
                .iter()
                .any(|model| provider_group(&model.id) == *provider)
            {
                tabs.push((*provider).to_string());
            }
        }
        tabs.push("catalog".to_string());
        let catalog_tab = tabs.len() - 1;
        let tab_index = |id: &str| -> usize {
            let group = provider_group(id);
            tabs.iter()
                .position(|name| name.as_str() == group)
                .unwrap_or(catalog_tab)
        };
        let widths = metric_widths(&models);
        items.extend(
            models
                .iter()
                .map(|model| model_row(model, active, &lang, &widths).tab(tab_index(&model.id))),
        );
        items.push(
            PickerItem::action("Show configured only", "/model").badge(tr(&lang, "badge.more")),
        );
        // Marks: "All" and every subscription provider are reachable (●); the
        // aggregate catalog tab is not (○), and the pane rules a line there.
        let marks = tabs
            .iter()
            .map(|name| name.as_str() != "catalog")
            .collect::<Vec<_>>();
        Ok(PickerSpec::new(tr(&lang, "view.model.title"), items)
            .with_tabs(tabs)
            .with_tab_marks(marks)
            .localized(&lang))
    } else {
        // Curated view, same two-pane shape as `--all`: the brands column is
        // the picker's left pane, so the provider rows are not repeated among
        // the models. `/model` is what people type; it must not be the one
        // view that stayed a flat list.
        models.retain(|model| model.available || active == Some(model.id.as_str()));
        models.sort_by(|left, right| {
            provider_rank(provider_group(&left.id))
                .cmp(&provider_rank(provider_group(&right.id)))
                .then_with(|| model_rank(left, active).cmp(&model_rank(right, active)))
                .then_with(|| left.id.cmp(&right.id))
        });
        let mut tabs = vec![tr(&lang, "picker.tab.all").to_string()];
        for provider in SUBSCRIPTION_PROVIDERS {
            if models
                .iter()
                .any(|model| provider_group(&model.id) == *provider)
            {
                tabs.push((*provider).to_string());
            }
        }
        let tab_index = |id: &str| -> usize {
            let group = provider_group(id);
            tabs.iter()
                .position(|name| name.as_str() == group)
                .unwrap_or_default()
        };
        let widths = metric_widths(&models);
        items.extend(
            models
                .iter()
                .map(|model| model_row(model, active, &lang, &widths).tab(tab_index(&model.id))),
        );
        items.push(catalog_row);
        items.push(
            PickerItem::action(format!("Show all {total} models"), "/model --all")
                .badge(tr(&lang, "badge.more")),
        );
        // Every tab here is a subscription the user holds, so all are ●; the
        // catalog stays a row in the item pane with its own ○.
        let marks = tabs.iter().map(|_| true).collect::<Vec<_>>();
        Ok(PickerSpec::new(tr(&lang, "view.model.title"), items)
            .with_tabs(tabs)
            .with_tab_marks(marks)
            .localized(&lang))
    }
}
