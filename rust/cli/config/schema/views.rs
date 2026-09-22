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

pub(super) fn config_list_text(cwd: &Path) -> String {
    let list = config_list_json(cwd);
    let lang = crate::cli::i18n::lang_code(cwd);
    let mut lines = vec![
        crate::cli::i18n::tr(&lang, "view.settings.title").to_string(),
        format!("Config: {}", user_config_path().display()),
    ];
    let mut current_group = "";
    for spec in settings_schema() {
        let group = spec.key.split('.').next().unwrap_or("settings");
        if group != current_group {
            current_group = group;
            lines.push(format!("\n[{group}]"));
        }
        let value = &list[spec.key]["value"];
        let value_text = value
            .as_str()
            .map(str::to_string)
            .unwrap_or_else(|| value.to_string());
        lines.push(format!(
            "{} = {} ({}) - {}",
            spec.key, value_text, spec.typ, spec.description
        ));
    }
    lines.join("\n") + "\n"
}

/// Group rows by top-level key prefix into picker tabs: known prefixes in
/// first-seen order, anything else under `other` after them. Row order within
/// a group is kept. Returns the tab bar (index 0 = the catch-all "All" view)
/// and the rows tagged with their 1-based tab index.
fn grouped_setting_rows(
    rows: Vec<(&str, PickerItem)>,
    lang: &str,
) -> (Vec<String>, Vec<PickerItem>) {
    const KNOWN_PREFIXES: &[&str] = &[
        "tools",
        "commands",
        "startup",
        "context",
        "contracts",
        "communication",
        "rules",
        "hooks",
        "secrets",
        "ui",
    ];
    let mut groups: Vec<(&str, Vec<PickerItem>)> = Vec::new();
    for (prefix, item) in rows {
        let label = if KNOWN_PREFIXES.contains(&prefix) {
            prefix
        } else {
            "other"
        };
        match groups.iter_mut().find(|(name, _)| *name == label) {
            Some((_, items)) => items.push(item),
            None => groups.push((label, vec![item])),
        }
    }
    groups.sort_by_key(|(name, _)| usize::from(*name == "other"));
    let mut tabs = vec![crate::cli::i18n::tr(lang, "picker.tab.all").to_string()];
    let mut items = Vec::new();
    for (name, group_rows) in groups {
        tabs.push(name.to_string());
        let tab = tabs.len() - 1;
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
        let current_text = current
            .as_str()
            .map(str::to_string)
            .unwrap_or_else(|| current.to_string());
        let detail = format!("{} Current: {current_text}.", spec.description);
        let prefix = spec.key.split('.').next().unwrap_or("other");
        match spec.typ {
            "boolean" => {
                let next = !current.as_bool().unwrap_or(false);
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
