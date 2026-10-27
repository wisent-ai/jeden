//! Showing the settings to a person: as text, as a list, and as picker rows.
//!
//! Split out of `cli/config/schema.rs`, which had grown past the module line
//! cap.

use super::super::merged_config_value;
use super::settings_schema;
use super::values::{effective_setting_value, setting_metadata};
use crate::cli::config::schema::values::setting_default;
use crate::tui::{PickerItem, PickerSpec};
use crate::user_config_path;
use serde_json::Value;
use std::path::Path;

pub(super) fn config_list_json(cwd: &Path) -> Value {
    let config = merged_config_value(cwd);
    let mut out = serde_json::Map::new();
    for spec in settings_schema() {
        out.insert(
            spec.key.to_string(),
            setting_metadata(spec, effective_setting_value(&config, spec)),
        );
    }
    Value::Object(out)
}

/// The group a setting key belongs to: the part before its first dot, or the
/// whole key when it has none.
fn group_of(key: &str) -> &str {
    match key.split_once('.') {
        Some((group, _rest)) => group,
        None => key,
    }
}

/// A setting value as a person reads it: a string without its quotes, any
/// other value as its JSON.
fn shown(value: &Value) -> String {
    match value {
        Value::String(text) => text.clone(),
        other => other.to_string(),
    }
}

pub(super) fn config_list_text(cwd: &Path) -> String {
    let list = config_list_json(cwd);
    let lang = crate::cli::i18n::lang_code(cwd);
    let mut lines = vec![
        crate::cli::i18n::tr(&lang, "view.settings.title").to_string(),
        format!("Config: {}", user_config_path().display()),
    ];
    let mut current_group = None;
    for spec in settings_schema() {
        let group = group_of(spec.key);
        if current_group != Some(group) {
            current_group = Some(group);
            lines.push(format!("\n[{group}]"));
        }
        lines.push(format!(
            "{} = {} ({}) - {}",
            spec.key,
            shown(&list[spec.key]["value"]),
            spec.typ,
            spec.description
        ));
    }
    lines.join("\n") + "\n"
}

/// Group rows by top-level key prefix into picker tabs, in the order the
/// schema first names each prefix. Row order within a group is kept. Returns
/// the tab bar (its first tab is the catch-all "All" view) and the rows tagged
/// with the index of their group's tab.
fn grouped_setting_rows(
    rows: Vec<(&str, PickerItem)>,
    lang: &str,
) -> (Vec<String>, Vec<PickerItem>) {
    let mut groups: Vec<(&str, Vec<PickerItem>)> = Vec::new();
    for (prefix, item) in rows {
        match groups.iter_mut().find(|(name, _)| *name == prefix) {
            Some((_, items)) => items.push(item),
            None => groups.push((prefix, vec![item])),
        }
    }
    let mut tabs = vec![crate::cli::i18n::tr(lang, "picker.tab.all").to_string()];
    let mut items = Vec::new();
    for (name, group_rows) in groups {
        let tab = tabs.len();
        tabs.push(name.to_string());
        items.extend(group_rows.into_iter().map(|item| item.tab(tab)));
    }
    (tabs, items)
}

pub(crate) fn settings_picker(cwd: &Path) -> PickerSpec {
    let (config, mut rows) = (merged_config_value(cwd), Vec::new());
    let lang = crate::cli::i18n::lang_code(cwd);
    for spec in settings_schema() {
        let (current, default) = (
            effective_setting_value(&config, spec),
            setting_default(spec),
        );
        let current_text = shown(&current);
        let detail = format!("{} Current: {current_text}.", spec.description);
        let prefix = group_of(spec.key);
        match spec.typ {
            "boolean" => {
                // The toggle sets true unless the setting already is true.
                let next = current != Value::Bool(true);
                rows.push((
                    prefix,
                    PickerItem::action(
                        format!("{}: set {next}", spec.key),
                        format!("/settings set {} {next}", spec.key),
                    )
                    .detail(&detail)
                    .badge(current_text.to_ascii_uppercase()),
                ));
            }
            "enum" => {
                for value in spec.enum_values {
                    let active = current.as_str() == Some(*value);
                    let item = PickerItem::action(
                        format!("{}: {value}", spec.key),
                        format!("/settings set {} {value}", spec.key),
                    )
                    .detail(&detail)
                    .disabled(active);
                    rows.push((
                        prefix,
                        if active {
                            item.badge(crate::cli::i18n::tr(&lang, "badge.active"))
                        } else {
                            item
                        },
                    ));
                }
            }
            _ => {
                // Number/string/array/record keys have no enumerable values:
                // offer a prefill row that drops `/settings set <key> ` into
                // the prompt so every schema key is editable from the picker.
                rows.push((
                    prefix,
                    PickerItem::action(
                        format!("{}: set value", spec.key),
                        format!("/settings set {} ", spec.key),
                    )
                    .detail(&detail)
                    .badge("INPUT")
                    .prefill(),
                ));
            }
        }
        if current != default {
            rows.push((
                prefix,
                PickerItem::action(
                    format!("{}: reset to default", spec.key),
                    format!("/settings reset {}", spec.key),
                )
                .detail(format!(
                    "{} Current: {current_text}. Default: {default}.",
                    spec.description
                ))
                .badge("RESET"),
            ));
        }
    }
    let (tabs, items) = grouped_setting_rows(rows, &lang);
    PickerSpec::new(crate::cli::i18n::tr(&lang, "view.settings.title"), items)
        .with_tabs(tabs)
        .localized(&lang)
}
