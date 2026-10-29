//! The marketplace rows an operator sees when the command is opened without
//! arguments. A field a source or plugin entry leaves out is named as left
//! out, and an entry without a name is shown as such rather than skipped.

use super::super::fetch::{catalog_plugins, read_marketplace_catalog};
use super::super::marketplace_cache_dir;
use super::super::registry::{plugin_registry, sorted_object_values};
use crate::slash::common::refusal_picker;
use crate::slash::SlashContext;
use crate::tui::{PickerItem, PickerSpec};
use serde_json::Value;

/// `field` of `entry`, or that the entry records none.
fn stated(entry: &Value, field: &str) -> String {
    match entry.get(field).and_then(Value::as_str) {
        Some(text) => text.to_string(),
        None => format!("no {field} recorded"),
    }
}

fn name_of(entry: &Value) -> Option<&str> {
    entry.get("name").and_then(Value::as_str)
}

/// A disabled row for an entry that cannot be offered, saying why.
fn unnamed_row(what: &str, entry: &Value) -> PickerItem {
    let mut row = PickerItem::action(format!("Unnamed {what}"), "")
        .detail(format!(
            "this {what} entry has no name, so it cannot be addressed: {entry}"
        ))
        .disabled(true);
    row.command = None;
    row
}

pub(crate) fn marketplace_picker(context: &SlashContext<'_>) -> PickerSpec {
    let lang = crate::cli::i18n::lang_code(context.cwd);
    let registries = plugin_registry(context.cwd)
        .and_then(|project| Ok((project, plugin_registry(&super::plugins_home())?)));
    let (project, user) = match registries {
        Ok(registries) => registries,
        Err(cause) => {
            return refusal_picker("Marketplace", "Plugin registry cannot be read", cause)
        }
    };
    let scoped_registries = [("project", project), ("user", user)];
    let mut items = Vec::new();
    for (source_scope, registry) in &scoped_registries {
        let mut sources = match sorted_object_values(&registry["sources"]) {
            Ok(sources) => sources,
            Err(cause) => {
                return refusal_picker("Marketplace", "Plugin registry cannot be read", cause)
            }
        };
        sources.sort_by(|a, b| name_of(a).cmp(&name_of(b)));
        for source in sources {
            let Some(name) = name_of(&source) else {
                items.push(unnamed_row(
                    &format!("{source_scope} marketplace source"),
                    &source,
                ));
                continue;
            };
            let location = stated(&source, "source");
            let source_enabled = source.get("enabled").and_then(Value::as_bool) != Some(false);
            items.push(
                PickerItem::action(
                    format!("Discover {name}"),
                    format!("/marketplace discover {name}"),
                )
                .detail(format!(
                    "{source_scope} source · {location} · {}",
                    if source_enabled {
                        "enabled"
                    } else {
                        "disabled"
                    }
                ))
                .badge("DISCOVER"),
            );
            let catalog = read_marketplace_catalog(&marketplace_cache_dir(name));
            let mut plugins = match catalog.and_then(|catalog| catalog_plugins(&catalog)) {
                Ok(plugins) => plugins,
                Err(cause) => {
                    return refusal_picker(
                        "Marketplace",
                        &format!("Catalogue for {name} cannot be read"),
                        cause,
                    );
                }
            };
            plugins.sort_by(|a, b| name_of(a).cmp(&name_of(b)));
            for plugin in plugins {
                let Some(plugin_name) = name_of(&plugin) else {
                    items.push(unnamed_row(&format!("plugin of {name}"), &plugin));
                    continue;
                };
                let id = format!("{plugin_name}@{name}");
                let description = stated(&plugin, "description");
                let available_version = stated(&plugin, "version");
                let installed = scoped_registries
                    .iter()
                    .filter_map(|(scope, scoped)| {
                        scoped["installed"].get(&id).map(|entry| (*scope, entry))
                    })
                    .collect::<Vec<_>>();
                if installed.is_empty() {
                    for install_scope in ["project", "user"] {
                        items.push(
                            PickerItem::action(
                                format!("Install {id} ({install_scope})"),
                                format!("/marketplace install --scope {install_scope} {id}"),
                            )
                            .detail(format!(
                                "{description} · {available_version} · {source_scope} source · not installed"
                            ))
                            .badge(crate::cli::i18n::tr(&lang, "badge.available")),
                        );
                    }
                    continue;
                }
                for (installed_scope, entry) in &installed {
                    let enabled = entry.get("enabled").and_then(Value::as_bool) != Some(false);
                    let installed_version = stated(entry, "version");
                    items.push(
                        PickerItem::action(
                            format!("Upgrade {id} ({installed_scope})"),
                            format!("/marketplace upgrade --scope {installed_scope} {id}"),
                        )
                        .detail(format!(
                            "{description} · installed {installed_version} in {installed_scope} scope · {} · source {location}",
                            if enabled { "enabled" } else { "disabled" }
                        ))
                        .badge("INSTALLED"),
                    );
                }
                items.push(
                    PickerItem::action(
                        format!("Uninstall {id} (all scopes)"),
                        format!("/marketplace uninstall {id}"),
                    )
                    .detail(format!(
                        "installed in {} · source {location}",
                        installed
                            .iter()
                            .map(|(scope, _)| *scope)
                            .collect::<Vec<_>>()
                            .join(" and ")
                    ))
                    .badge("DESTRUCTIVE"),
                );
            }
        }
    }
    items.push(
        PickerItem::action("Add marketplace source", "/marketplace add ")
            .detail("Edit the local path or catalog URL before submitting")
            .badge("INPUT")
            .prefill(),
    );
    PickerSpec::new("Marketplace", items)
}
