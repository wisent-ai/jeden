//! Ordering and labelling the rows of the model picker.
//!
//! Split out of `cli/run/slash_ui.rs`, which had grown past the module line
//! cap.

use super::columns::{context_metric, model_metrics, price_detail};
use crate::cli::i18n::tr;
use crate::control_plane::brama::ModelEntry;
use crate::tui::PickerItem;

/// Provider group: the id segment before the first `/`; ids without `/` group
/// under their literal id.
pub(super) fn provider_group(id: &str) -> &str {
    id.split_once('/').map(|(head, _)| head).unwrap_or(id)
}

/// Subscription providers summarized at the top of the picker, in fixed
/// order; everything else belongs to the public catalog.
const SUBSCRIPTION_PROVIDERS: &[&str] = &["claude-code", "codex", "kimi"];

/// Subscription providers first in a fixed order, everything else alphabetical.
pub(super) fn provider_rank(provider: &str) -> (u8, &str) {
    match SUBSCRIPTION_PROVIDERS
        .iter()
        .position(|known| *known == provider)
    {
        Some(index) => (index as u8, ""),
        None => (SUBSCRIPTION_PROVIDERS.len() as u8, provider),
    }
}

/// Active and credential-backed models first; the unavailable tail last.
pub(super) fn model_rank(model: &ModelEntry, active: Option<&str>) -> u8 {
    if !model.available {
        2
    } else if active == Some(model.id.as_str()) {
        0
    } else {
        1
    }
}

pub(super) fn model_row(model: &ModelEntry, active: Option<&str>, lang: &str, widths: &[usize]) -> PickerItem {
    let selected = active == Some(model.id.as_str());
    let detail = format!(
        "context {} · output {} · {}{}",
        model.context_window,
        model.max_output_tokens,
        if model.tools { "tools" } else { "no tools" },
        if model.reasoning { " · reasoning" } else { "" }
    );
    let detail = model.unavailable_reason.clone().unwrap_or(detail);
    // The row shows figures in right-aligned columns (perf · context · price,
    // omp's order); the prose stays in `detail`, which the pane prints under
    // the list for whatever the cursor is on.
    PickerItem::action(&model.id, format!("/model {}", model.id))
        .detail(detail)
        .metrics(model_metrics(model, widths))
        .badge(if !model.available {
            tr(lang, "badge.unavailable")
        } else if selected {
            tr(lang, "badge.active")
        } else {
            tr(lang, "badge.available")
        })
        .disabled(selected || !model.available)
}

/// Disabled, command-less summary row (● subscription / ○ catalog); like a
/// group header, the picker skips it. The label carries the state dot — no
/// badge, so the dot never renders twice.
pub(super) fn summary_row(label: String, detail: String) -> PickerItem {
    let mut row = PickerItem::action(label, "").detail(detail);
    row.command = None;
    row.disabled = true;
    row
}

/// Pick the plural-form i18n key for a `N models` summary row. English and
/// the generated overlays only distinguish one/many; Polish hand rows also
/// carry the 2–4 "few" form (`model`/`modele`/`modeli`).
pub(super) fn summary_key(base: &'static str, lang: &str, count: usize) -> &'static str {
    if count == 1 {
        match base {
            "picker.summary.subscription" => return "picker.summary.subscription.one",
            "picker.summary.catalog" => return "picker.summary.catalog.one",
            _ => return base,
        }
    }
    let few =
        lang == "pl" && (2..=4).contains(&(count % 10)) && !(12..=14).contains(&(count % 100));
    if few {
        match base {
            "picker.summary.subscription" => "picker.summary.subscription.few",
            "picker.summary.catalog" => "picker.summary.catalog.few",
            _ => base,
        }
    } else {
        base
    }
}
