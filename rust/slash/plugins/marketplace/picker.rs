//! The marketplace rows an operator sees when the command is opened without
//! arguments.
//!
//! Split out of `slash/plugins/marketplace.rs`, which had grown past the
//! module line cap.

use super::super::fetch::{catalog_plugins, read_marketplace_catalog};
use super::super::marketplace_cache_dir;
use super::super::registry::{plugin_registry, sorted_object_values};
use crate::slash::SlashContext;
use crate::tui::{PickerItem, PickerSpec};
use serde_json::Value;

pub(crate) fn marketplace_picker(context: &SlashContext<'_>) -> PickerSpec {
    let lang = crate::cli::i18n::lang_code(context.cwd);
    let scoped_registries = [
        ("project", plugin_registry(context.cwd)),
        ("user", plugin_registry(&super::plugins_home())),
    ];
    let mut items = Vec::new();
    for (source_scope, registry) in &scoped_registries {
        let mut sources = sorted_object_values(&registry["sources"]);
        sources.sort_by(|a, b| {
            a.get("name")
                .and_then(Value::as_str)
                .unwrap_or("")
                .cmp(b.get("name").and_then(Value::as_str).unwrap_or(""))
        });
        for source in sources {
            let Some(name) = source.get("name").and_then(Value::as_str) else {
                continue;
            };
            let location = source
                .get("source")
                .and_then(Value::as_str)
                .unwrap_or("unknown source");
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
            let mut plugins = read_marketplace_catalog(&marketplace_cache_dir(name))
                .map(|catalog| catalog_plugins(&catalog))
                .unwrap_or_else(|_| {
                    source
                        .get("plugins")
                        .and_then(Value::as_array)
                        .cloned()
                        .unwrap_or_default()
                });
            plugins.sort_by(|a, b| {
                a.get("name")
                    .and_then(Value::as_str)
                    .unwrap_or("")
                    .cmp(b.get("name").and_then(Value::as_str).unwrap_or(""))
            });
            for plugin in plugins {
                let Some(plugin_name) = plugin.get("name").and_then(Value::as_str) else {
                    continue;
                };
                let id = format!("{plugin_name}@{name}");
                let description = plugin
                    .get("description")
                    .and_then(Value::as_str)
                    .unwrap_or("");
                let available_version = plugin
                    .get("version")
                    .and_then(Value::as_str)
                    .unwrap_or("unknown version");
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
                    let installed_version = entry
                        .get("version")
                        .and_then(Value::as_str)
                        .unwrap_or("unknown version");
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
