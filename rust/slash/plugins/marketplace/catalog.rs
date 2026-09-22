//! The half of the marketplace command that deals with plugins rather than
//! with the sources they come from.
//!
//! Split out of `slash/plugins/marketplace.rs`, which had grown past the
//! module line cap.

use super::super::fetch::{catalog_plugins, fetch_marketplace, read_marketplace_catalog};
use super::super::ops::{all_marketplace_sources, find_marketplace_source, install_one, merged_installed_values, normalize_scope, parse_marketplace_flags, split_plugin_id, update_source_plugins};
use super::super::registry::{format_plugin, save_plugin_registry};
use crate::slash::SlashContext;
use serde_json::Value;
use crate::slash::support::common::dirs_home;
use super::super::marketplace_cache_dir;
use super::super::ops::{installed_entries_for_scope, registry_scope_dir};
use super::super::registry::plugin_registry;

pub(super) fn handle_catalog(
    verb: &str,
    rest: &[String],
    first: &str,
    context: &SlashContext<'_>,
) -> Result<String, String> {
    if verb == "installed" {
        let mut installed = merged_installed_values(context.cwd);
        installed.sort_by(|a, b| {
            a.get("id")
                .and_then(Value::as_str)
                .unwrap_or("")
                .cmp(b.get("id").and_then(Value::as_str).unwrap_or(""))
        });
        return Ok(if installed.is_empty() {
            "No plugins installed.".into()
        } else {
            ["Installed plugins:".into()]
                .into_iter()
                .chain(installed.iter().map(format_plugin))
                .collect::<Vec<_>>()
                .join("\n")
        });
    }
    if verb == "uninstall" {
        if first.is_empty() {
            return Err("Usage: /marketplace uninstall <name@marketplace>".into());
        }
        let mut removed_from = None;
        for dir in [context.cwd.to_path_buf(), dirs_home()] {
            let mut scoped = plugin_registry(&dir);
            let removed = scoped
                .get_mut("installed")
                .and_then(Value::as_object_mut)
                .and_then(|m| m.remove(first));
            if removed.is_some() {
                let file = save_plugin_registry(&dir, &scoped)?;
                removed_from = Some(file);
            }
        }
        match removed_from {
            Some(file) => {
                return Ok(format!(
                    "Uninstalled plugin {} from {}.",
                    first,
                    file.display()
                ))
            }
            None => return Err(format!("Installed plugin not found: {first}")),
        }
    }
    if verb == "discover" {
        let sources: Vec<(String, String)> = if first.is_empty() {
            all_marketplace_sources(context.cwd)
        } else {
            match find_marketplace_source(context.cwd, first) {
                Some(src) => vec![(first.to_string(), src)],
                None => return Err(format!("Marketplace source not found: {first}")),
            }
        };
        if sources.is_empty() {
            return Ok(
                "No marketplace sources configured. Add one with /marketplace add <source>.".into(),
            );
        }
        let mut lines = vec!["Available plugins:".to_string()];
        let mut any = false;
        let mut errors = Vec::new();
        for (name, source) in sources {
            let cache = marketplace_cache_dir(&name);
            if !cache.exists() {
                if let Err(error) = fetch_marketplace(context.cwd, &name, &source) {
                    errors.push(format!("- {name}: {error}"));
                    continue;
                }
            }
            let catalog = match read_marketplace_catalog(&cache) {
                Ok(catalog) => catalog,
                Err(error) => {
                    errors.push(format!("- {name}: {error}"));
                    continue;
                }
            };
            let mut plugins = catalog_plugins(&catalog);
            plugins.sort_by(|a, b| {
                a.get("name")
                    .and_then(Value::as_str)
                    .unwrap_or("")
                    .cmp(b.get("name").and_then(Value::as_str).unwrap_or(""))
            });
            for plugin in plugins {
                let pname = plugin.get("name").and_then(Value::as_str).unwrap_or("-");
                let desc = plugin
                    .get("description")
                    .and_then(Value::as_str)
                    .unwrap_or("");
                lines.push(format!("{pname}@{name}\t{desc}"));
                any = true;
            }
        }
        if !any {
            lines.push("- none".into());
        }
        lines.extend(errors);
        return Ok(lines.join("\n"));
    }
    if verb == "update" {
        let sources: Vec<(String, String)> = if first.is_empty() {
            all_marketplace_sources(context.cwd)
        } else {
            match find_marketplace_source(context.cwd, first) {
                Some(src) => vec![(first.to_string(), src)],
                None => return Err(format!("Marketplace source not found: {first}")),
            }
        };
        if sources.is_empty() {
            return Ok(
                "No marketplace sources configured. Add one with /marketplace add <source>.".into(),
            );
        }
        let mut updated = Vec::new();
        let mut counts = Vec::new();
        let mut errors = Vec::new();
        for (name, source) in sources {
            match fetch_marketplace(context.cwd, &name, &source)
                .and_then(|cache| read_marketplace_catalog(&cache))
            {
                Ok(catalog) => {
                    let plugins = catalog_plugins(&catalog);
                    counts.push(plugins.len());
                    update_source_plugins(context.cwd, &name, &plugins);
                    updated.push(name);
                }
                Err(error) => errors.push(format!("- {name}: {error}")),
            }
        }
        let total_plugins: usize = counts.iter().sum();
        let mut out = vec![format!(
            "Updated {} marketplace source(s), {total_plugins} plugin(s) available.",
            updated.len()
        )];
        out.extend(errors);
        return Ok(out.join("\n"));
    }
    if verb == "install" {
        let (force, scope, targets) = parse_marketplace_flags(rest);
        let scope = normalize_scope(scope)?;
        if targets.is_empty() {
            return Err(
                "Usage: /marketplace install [--force] [--scope user|project] <name@marketplace>"
                    .into(),
            );
        }
        let mut lines = Vec::new();
        for target in targets {
            let (plugin, mkt) = split_plugin_id(&target)?;
            lines.push(install_one(context.cwd, &mkt, &plugin, &scope, force)?);
        }
        return Ok(lines.join("\n"));
    }
    if verb == "upgrade" {
        let (_force, scope, targets) = parse_marketplace_flags(rest);
        let scoped = match scope.as_deref() {
            Some(s) => vec![normalize_scope(Some(s.to_string()))?],
            None => vec!["project".to_string(), "user".to_string()],
        };
        let mut ids: Vec<(String, String)> = Vec::new();
        if targets.is_empty() {
            for scope in &scoped {
                for entry in installed_entries_for_scope(&registry_scope_dir(context.cwd, scope)) {
                    if let Some(id) = entry.get("id").and_then(Value::as_str) {
                        ids.push((id.to_string(), scope.clone()));
                    }
                }
            }
        } else {
            let scope = normalize_scope(scope)?;
            for target in targets {
                ids.push((target, scope.clone()));
            }
        }
        if ids.is_empty() {
            return Ok("No installed plugins to upgrade.".into());
        }
        let mut lines = Vec::new();
        for (id, scope) in ids {
            let (plugin, mkt) = split_plugin_id(&id)?;
            lines.push(install_one(context.cwd, &mkt, &plugin, &scope, true)?);
        }
        return Ok(lines.join("\n"));
    }
    Err("Usage: /marketplace add <source> | remove <name> | list | update [name] | discover [marketplace] | install <name@marketplace> | upgrade [name@marketplace] | installed | uninstall <name@marketplace> | help".into())
}
