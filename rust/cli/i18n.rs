//! First i18n layer for UI chrome: a static string table keyed by
//! (language code, key) with English as the ultimate fallback. Contract-critical
//! agent syntax (Rules block, action protocol, tool registry) never goes
//! through this table and stays English always.

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
];

/// Look up `key` for `lang`: hand-written rows first, then the generated
/// overlay in `i18n_translations`, then the English row; a key missing even
/// from English yields the key itself. Unknown languages (including `auto`)
/// fall back to English. This never panics.
pub(crate) fn tr(lang: &str, key: &'static str) -> &'static str {
    STRINGS
        .iter()
        .find(|(row_lang, row_key, _)| *row_lang == lang && *row_key == key)
        .or_else(|| {
            super::i18n_translations::GENERATED_TRANSLATIONS
                .iter()
                .find(|(row_lang, row_key, _)| *row_lang == lang && *row_key == key)
        })
        .or_else(|| {
            STRINGS
                .iter()
                .find(|(row_lang, row_key, _)| *row_lang == "en" && *row_key == key)
        })
        .map(|(_, _, text)| *text)
        .unwrap_or(key)
}

/// Resolve the chrome language code for a workspace from `ui.language`.
/// `auto` (and any code without table rows) resolves to English inside `tr`.
pub(crate) fn lang_code(cwd: &Path) -> String {
    ui_language(&load_config(cwd)).code().to_string()
}
