use std::io::IsTerminal;
use std::path::Path;

use crate::cli::auth::{format_auth_status, provider_picker};
use crate::cli::config::{load_config, schema::settings_picker, ui_language};
use crate::cli::i18n::{lang_code, tr};
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

/// Price suffix in the oh-my-pi convention: priced models end with a `█` bar
/// ($5/30█, $0.25/1.25█), free models with `free│`.
fn price_detail(price: &ModelPrice) -> String {
    if price.input > 0.0 || price.output > 0.0 {
        format!(
            " · ${}/{}█",
            format_price(price.input),
            format_price(price.output)
        )
    } else {
        " · free│".into()
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

/// Subscription providers summarized at the top of the picker, in fixed
/// order; everything else belongs to the public catalog.
const SUBSCRIPTION_PROVIDERS: &[&str] = &["claude-code", "codex", "kimi"];

/// Subscription providers first in a fixed order, everything else alphabetical.
fn provider_rank(provider: &str) -> (u8, &str) {
    match SUBSCRIPTION_PROVIDERS.iter().position(|known| *known == provider) {
        Some(index) => (index as u8, ""),
        None => (SUBSCRIPTION_PROVIDERS.len() as u8, provider),
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

fn model_row(model: &ModelEntry, active: Option<&str>, lang: &str) -> PickerItem {
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
fn summary_row(label: String, detail: String) -> PickerItem {
    let mut row = PickerItem::action(label, "").detail(detail);
    row.command = None;
    row.disabled = true;
    row
}

/// Pick the plural-form i18n key for a `N models` summary row. English and
/// the generated overlays only distinguish one/many; Polish hand rows also
/// carry the 2–4 "few" form (`model`/`modele`/`modeli`).
fn summary_key(base: &'static str, lang: &str, count: usize) -> &'static str {
    if count == 1 {
        match base {
            "picker.summary.subscription" => return "picker.summary.subscription.one",
            "picker.summary.catalog" => return "picker.summary.catalog.one",
            _ => return base,
        }
    }
    let few = lang == "pl"
        && (2..=4).contains(&(count % 10))
        && !(12..=14).contains(&(count % 100));
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

pub(crate) fn model_picker(
    cwd: &Path,
    current_model: Option<&str>,
    show_all: bool,
) -> Result<PickerSpec, String> {
    let config = load_config(cwd);
    let lang = ui_language(&config).code().to_string();
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
                tr(&lang, summary_key("picker.summary.subscription", &lang, count))
                    .replace("{}", &count.to_string()),
            ));
        }
    }
    let remainder = total.saturating_sub(covered);
    let catalog_row = summary_row(
        "○ catalog".to_string(),
        tr(&lang, summary_key("picker.summary.catalog", &lang, remainder))
            .replace("{}", &remainder.to_string()),
    );
    if show_all {
        items.extend(summary);
        items.push(catalog_row);
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
            tabs
                .iter()
                .position(|name| name.as_str() == group)
                .unwrap_or(catalog_tab)
        };
        items.extend(
            models
                .iter()
                .map(|model| model_row(model, active, &lang).tab(tab_index(&model.id))),
        );
        items.push(
            PickerItem::action("Show configured only", "/model").badge(tr(&lang, "badge.more")),
        );
        return Ok(PickerSpec::new(tr(&lang, "view.model.title"), items)
            .with_tabs(tabs)
            .localized(&lang));
    } else {
        items.extend(summary);
        models.retain(|model| model.available || active == Some(model.id.as_str()));
        models.sort_by(|left, right| {
            model_rank(left, active)
                .cmp(&model_rank(right, active))
                .then_with(|| left.id.cmp(&right.id))
        });
        items.extend(models.iter().map(|model| model_row(model, active, &lang)));
        items.push(catalog_row);
        items.push(
            PickerItem::action(format!("Show all {total} models"), "/model --all")
                .badge(tr(&lang, "badge.more")),
        );
    }
    Ok(PickerSpec::new(tr(&lang, "view.model.title"), items).localized(&lang))
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
            let lang = lang_code(cwd);
            return Some(Ok(CommandOutcome::Picker(PickerSpec::new(
                format!("{} unavailable", view.ui.label),
                vec![PickerItem::action(view.ui.label, "")
                    .detail(detail)
                    .badge(tr(&lang, "badge.unavailable"))
                    .disabled(true)],
            ))));
        }
    }
    match command {
        "/login" => {
            return Some(Ok(CommandOutcome::Text(format_auth_status(cwd))));
        }
        "/setup" | "/onboarding"
            if !std::io::stdin().is_terminal() || !std::io::stdout().is_terminal() =>
        {
            // Piped stdin/stdout: print the manual checklist instead of a view.
            let session_root = crate::session_root();
            let context = SlashContext {
                cwd,
                model,
                session_root: &session_root,
            };
            return Some(crate::slash::setup::handle_text("", &context)
                .map(CommandOutcome::Text));
        }
        "/providers" => {
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


#[cfg(test)]
mod tests {
    #[test]
    fn summary_key_picks_english_singular_and_plural() {
        assert_eq!(
            super::summary_key("picker.summary.subscription", "en", 1),
            "picker.summary.subscription.one"
        );
        assert_eq!(
            super::summary_key("picker.summary.subscription", "en", 2),
            "picker.summary.subscription"
        );
        assert_eq!(
            super::summary_key("picker.summary.catalog", "en", 1),
            "picker.summary.catalog.one"
        );
    }

    #[test]
    fn summary_key_picks_polish_few_form() {
        // Polish: 1 model, 2–4 modele, 5+ modeli; 12–14 stay modeli.
        assert_eq!(
            super::summary_key("picker.summary.subscription", "pl", 1),
            "picker.summary.subscription.one"
        );
        for n in [2_usize, 3, 4, 22, 33] {
            assert_eq!(
                super::summary_key("picker.summary.subscription", "pl", n),
                "picker.summary.subscription.few",
                "count {n}"
            );
        }
        for n in [5_usize, 11, 12, 14, 25] {
            assert_eq!(
                super::summary_key("picker.summary.subscription", "pl", n),
                "picker.summary.subscription",
                "count {n}"
            );
        }
    }

    #[test]
    fn summary_key_ignores_few_for_non_polish() {
        assert_eq!(
            super::summary_key("picker.summary.catalog", "de", 3),
            "picker.summary.catalog"
        );
    }

    #[test]
    fn summary_keys_resolve_to_translated_text() {
        // Every key the helper can return must resolve for the language that
        // can select it (`.few` is Polish-only; `.one`/base fall back to en).
        let cases: &[(&str, &[&str])] = &[
            ("picker.summary.subscription", &["en", "pl"]),
            ("picker.summary.subscription.one", &["en", "pl"]),
            ("picker.summary.subscription.few", &["pl"]),
            ("picker.summary.catalog", &["en", "pl"]),
            ("picker.summary.catalog.one", &["en", "pl"]),
            ("picker.summary.catalog.few", &["pl"]),
        ];
        for (key, langs) in cases {
            for lang in *langs {
                let text = crate::cli::i18n::tr(lang, key);
                assert!(
                    text.contains("{}") && !text.starts_with("picker."),
                    "{lang}/{key} unresolved: {text}"
                );
            }
        }
    }
}
