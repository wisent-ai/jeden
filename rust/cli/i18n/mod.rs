//! The i18n layer for UI chrome: a static string table keyed by (language
//! code, key) with English as the ultimate fallback. Every key reads in every
//! language a person can pin (the fleet list from wisent-texts, see
//! `config::ui_language_codes`), which the `every_language` test holds.
//! Contract-critical agent syntax (Rules block, action protocol, tool
//! registry) never goes through this table and stays English always.

mod translations;

use std::path::Path;

use super::config::{load_config, ui_language};

/// `(language code, key, text)` rows. Every key must have an `en` row; other
/// languages are partial overlays that fall back to English per key.
static STRINGS: &[(&str, &str, &str)] = &[
    ("en", "picker.search_placeholder", "Type to search:"),
    ("pl", "picker.search_placeholder", "Wpisz, aby wyszukać:"),
    (
        "en",
        "picker.footer",
        "↑↓ select  Home/End jump  Enter confirm  Ctrl-U clear  Esc close",
    ),
    (
        "pl",
        "picker.footer",
        "↑↓ wybierz  Home/End skocz  Enter zatwierdź  Ctrl-U wyczyść  Esc zamknij",
    ),
    (
        "en",
        "picker.summary.subscription",
        "{} models · your subscription",
    ),
    (
        "pl",
        "picker.summary.subscription",
        "{} modeli · Twoja subskrypcja",
    ),
    (
        "en",
        "picker.summary.subscription.one",
        "{} model · your subscription",
    ),
    (
        "pl",
        "picker.summary.subscription.one",
        "{} model · Twoja subskrypcja",
    ),
    (
        "pl",
        "picker.summary.subscription.few",
        "{} modele · Twoja subskrypcja",
    ),
    ("en", "picker.summary.catalog", "{} models · no credentials"),
    (
        "pl",
        "picker.summary.catalog",
        "{} modeli · bez danych logowania",
    ),
    (
        "en",
        "picker.summary.catalog.one",
        "{} model · no credentials",
    ),
    (
        "pl",
        "picker.summary.catalog.one",
        "{} model · bez danych logowania",
    ),
    (
        "pl",
        "picker.summary.catalog.few",
        "{} modele · bez danych logowania",
    ),
    ("en", "view.model.title", "Select model route"),
    ("pl", "view.model.title", "Wybierz trasę modelu"),
    ("en", "view.settings.title", "Jeden settings"),
    ("pl", "view.settings.title", "Ustawienia Jeden"),
    ("en", "view.usage.title", "Provider usage"),
    ("pl", "view.usage.title", "Zużycie dostawców"),
    ("en", "view.session.title", "Session workflow"),
    ("pl", "view.session.title", "Przepływ pracy sesji"),
    ("en", "view.roles.title", "Model roles"),
    ("pl", "view.roles.title", "Role modeli"),
    ("en", "view.agents.title", "Agents"),
    ("pl", "view.agents.title", "Agenci"),
    ("en", "view.confirm.title", "Confirm destructive action"),
    ("pl", "view.confirm.title", "Potwierdź destrukcyjną akcję"),
    ("en", "badge.active", "ACTIVE"),
    ("pl", "badge.active", "AKTYWNY"),
    ("en", "badge.available", "AVAILABLE"),
    ("pl", "badge.available", "DOSTĘPNY"),
    ("en", "badge.unavailable", "UNAVAILABLE"),
    ("pl", "badge.unavailable", "NIEDOSTĘPNY"),
    ("en", "badge.auto", "AUTO"),
    ("pl", "badge.auto", "AUTO"),
    ("en", "badge.more", "MORE"),
    ("pl", "badge.more", "WIĘCEJ"),
    ("en", "badge.current", "CURRENT"),
    ("pl", "badge.current", "BIEŻĄCY"),
    ("en", "badge.default", "DEFAULT"),
    ("pl", "badge.default", "DOMYŚLNY"),
    ("en", "badge.custom", "CUSTOM"),
    ("pl", "badge.custom", "WŁASNY"),
    ("en", "picker.footer.tabs", "Tab category"),
    ("pl", "picker.footer.tabs", "Tab kategoria"),
    (
        "en",
        "picker.footer.panes",
        "↑↓ groups  → items  ← back  Enter confirm  type to search  Esc close",
    ),
    (
        "pl",
        "picker.footer.panes",
        "↑↓ grupy  → pozycje  ← wróć  Enter zatwierdź  pisz aby szukać  Esc zamknij",
    ),
    ("en", "picker.tab.all", "All"),
    ("pl", "picker.tab.all", "Wszystkie"),
    (
        "en",
        "star.request",
        "★ You have sent {prompts} prompts with Jeden. If it helps you, a star on GitHub helps other people find it: {repository}\n`/star dismiss` stops this message.",
    ),
    (
        "pl",
        "star.request",
        "★ Wysłałeś już {prompts} promptów w Jedenie. Jeśli Ci pomaga, gwiazdka na GitHubie pomoże innym go znaleźć: {repository}\n`/star dismiss` wyłącza tę wiadomość.",
    ),
];

/// Look up `key` for `lang`: hand-written rows first, then the generated
/// overlay in `i18n::translations`, then the English row. Unknown languages
/// (including `auto`) read English. Every key the code names has an English row;
/// one without is a defect in this table, stopped naming the key rather than
/// shown to the person as the key itself.
pub(crate) fn tr(lang: &str, key: &'static str) -> &'static str {
    let row = STRINGS
        .iter()
        .find(|(row_lang, row_key, _)| *row_lang == lang && *row_key == key)
        .or_else(|| {
            translations::GENERATED_TRANSLATIONS
                .iter()
                .find(|(row_lang, row_key, _)| *row_lang == lang && *row_key == key)
        })
        .or_else(|| {
            STRINGS
                .iter()
                .find(|(row_lang, row_key, _)| *row_lang == "en" && *row_key == key)
        });
    match row {
        Some((_, _, text)) => text,
        None => panic!("i18n key {key:?} has no English row in cli/i18n STRINGS"),
    }
}

/// Resolve the chrome language code for a workspace from `ui.language`.
/// `auto` (and any code without table rows) resolves to English inside `tr`.
pub(crate) fn lang_code(cwd: &Path) -> String {
    ui_language(&load_config(cwd)).code().to_string()
}

#[cfg(test)]
mod every_language {
    use super::{translations::GENERATED_TRANSLATIONS, STRINGS};

    /// The placeholders a text carries: `{}` and `{name}`, in order.
    fn placed(text: &str) -> Vec<&str> {
        let mut found = Vec::new();
        let mut rest = text;
        while let Some(start) = rest.find('{') {
            let Some(end) = rest[start..].find('}') else { break };
            found.push(&rest[start..start + end + 1]);
            rest = &rest[start + end + 1..];
        }
        found.sort_unstable();
        found
    }

    /// Every English key reads in every language a person can pin (the fleet
    /// list), and keeps the values its English text places; a `.few` plural
    /// form only some languages use is not demanded of the others.
    #[test]
    fn every_key_reads_in_every_fleet_language_with_its_placeholders() {
        let rows = STRINGS.iter().chain(GENERATED_TRANSLATIONS.iter());
        let mut missing = Vec::new();
        let mut misplaced = Vec::new();
        let english: Vec<_> = STRINGS.iter().filter(|(lang, _, _)| *lang == "en").collect();
        for language in crate::cli::config::ui_language_codes() {
            for (_, key, source) in &english {
                match rows.clone().find(|(lang, row_key, _)| lang == language && row_key == key) {
                    None => missing.push(format!("{language}/{key}")),
                    Some((_, _, text)) if placed(text) != placed(source) => {
                        misplaced.push(format!("{language}/{key}: {text}"))
                    }
                    Some(_) => {}
                }
            }
        }
        assert!(missing.is_empty(), "keys without a row in their language: {missing:?}");
        assert!(misplaced.is_empty(), "rows that drop or add a placeholder: {misplaced:?}");
    }
}
