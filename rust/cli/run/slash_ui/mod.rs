//! The interactive views the run command can open.

use std::io::IsTerminal;
use std::path::Path;

use crate::cli::auth::{format_auth_status, provider_picker};
use crate::cli::config::schema::settings_picker;
use crate::cli::i18n::lang_code;
use crate::slash::{self, SlashContext};
use crate::tui::{CommandOutcome, PickerItem, PickerSpec};

mod columns;
mod picker;
mod rows;

pub(crate) use picker::model_picker;
use crate::cli::i18n::tr;


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
    if command == "/onboarding" {
        return Some(crate::onboarding::interactive(args.trim(), cwd));
    }
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
        "/setup" if !std::io::stdin().is_terminal() || !std::io::stdout().is_terminal() => {
            // Piped stdin/stdout: print the manual checklist instead of a view.
            let session_root = crate::session_root();
            let context = SlashContext {
                cwd,
                model,
                session_root: &session_root,
            };
            return Some(crate::slash::setup::handle_text("", &context).map(CommandOutcome::Text));
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
