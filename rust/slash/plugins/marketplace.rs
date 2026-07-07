use serde_json::{json, Value};
use std::fs;

use crate::slash::common::{dirs_home, now_text, split_args};
use crate::slash::validate::valid_marketplace_name;
use crate::slash::SlashContext;
use super::fetch::{catalog_plugins, fetch_marketplace, read_marketplace_catalog};
use super::ops::{all_marketplace_sources, find_marketplace_source, install_one, installed_entries_for_scope, merged_installed_values, normalize_scope, parse_marketplace_flags, registry_scope_dir, split_plugin_id, update_source_plugins};
use super::registry::{format_plugin, format_plugin_source, plugin_registry, save_plugin_registry, sorted_object_values};
use super::marketplace_cache_dir;

/// Sanitize a string into a marketplace/version cache component: keep
/// alphanumerics and `.`/`_`/`-`, collapse other runs to a single `-`, trim
/// leading/trailing `-`. Returns an empty string when nothing survives (callers
/// treat an empty result as invalid rather than substituting a default).
pub(crate) fn sanitize_marketplace_name(value: &str) -> String {
    let mut out = String::new();
    for ch in value.trim().chars() {
        if ch.is_ascii_alphanumeric() || ch == '.' || ch == '_' || ch == '-' {
            out.push(ch);
        } else if matches!(ch, '@' | '/' | '\\') {
            out.push('-');
        } else if !out.ends_with('-') {
            out.push('-');
        }
    }
    out.trim_matches('-').to_string()
}

fn marketplace_source_name(source: &str) -> String {
    let text = source.trim().trim_end_matches('/');
    let tail = text.rsplit('/').next().unwrap_or(text);
    sanitize_marketplace_name(if tail.is_empty() { text } else { tail })
}

fn marketplace_source_type(source: &str) -> &'static str {
    let text = source.trim().to_ascii_lowercase();
    if text.starts_with("http://") || text.starts_with("https://") { "url" }
    else if text.starts_with("ssh://") || text.starts_with("git+ssh://") || text.starts_with("git@") { "git" }
    else { "local" }
}

pub(crate) fn handle_marketplace(args: &str, context: &SlashContext<'_>) -> Result<String, String> {
    let argv = split_args(args);
    let verb = argv.first().map(String::as_str).unwrap_or("help");
    let rest = argv.split_first().map(|(_, r)| r).unwrap_or(&[]);
    let first = rest.first().map(String::as_str).unwrap_or("");
    let mut registry = plugin_registry(context.cwd);
    if verb == "help" {
        return Ok("Usage: /marketplace add <source> | remove <name> | list | update [name] | discover [marketplace] | install [--force] [--scope user|project] <name@marketplace> | upgrade [--scope user|project] [name@marketplace] | installed | uninstall <name@marketplace>.".into());
    }
    if verb == "add" {
        let source = rest.iter().cloned().collect::<Vec<_>>().join(" ").trim().to_string();
        if source.is_empty() { return Err("Usage: /marketplace add <source>".into()); }
        let provisional = marketplace_source_name(&source);
        if !valid_marketplace_name(&provisional) {
            return Err(format!("Cannot derive a valid marketplace name from '{source}'."));
        }
        // OMP keys a marketplace by its catalog `name`. Fetch + read the catalog
        // authoritatively (errors surface), then rekey the cache to that name.
        let cache = fetch_marketplace(context.cwd, &provisional, &source)?;
        let catalog = read_marketplace_catalog(&cache)?;
        let cn = catalog.get("name").and_then(Value::as_str).unwrap_or("").trim().to_string();
        let name = if valid_marketplace_name(&cn) {
            if cn != provisional {
                let to = marketplace_cache_dir(&cn);
                if let Some(parent) = to.parent() { let _ = fs::create_dir_all(parent); }
                let _ = fs::remove_dir_all(&to);
                fs::rename(&cache, &to).map_err(|e| format!("failed to key marketplace cache to '{cn}': {e}"))?;
            }
            cn
        } else {
            return Err(format!("Marketplace catalog at '{source}' has an invalid or missing name."));
        };
        let existing = registry.get("sources").and_then(Value::as_object).and_then(|sources| sources.get(&name)).cloned().unwrap_or_else(|| json!({}));
        let added_at = existing.get("addedAt").cloned().unwrap_or_else(|| json!(now_text()));
        registry.get_mut("sources").and_then(Value::as_object_mut).ok_or("invalid plugin registry")?.insert(name.clone(), json!({
            "name": name,
            "source": source.clone(),
            "type": marketplace_source_type(&source),
            "enabled": true,
            "addedAt": added_at,
            "updatedAt": now_text(),
            "plugins": existing.get("plugins").filter(|value| value.is_array()).cloned().unwrap_or_else(|| json!([])),
        }));
        let file = save_plugin_registry(context.cwd, &registry)?;
        return Ok(format!("Added marketplace source {} ({}) in {}.", name, source, file.display()));
    }
    if verb == "remove" {
        if first.is_empty() { return Err("Usage: /marketplace remove <name>".into()); }
        let sources = registry.get_mut("sources").and_then(Value::as_object_mut).ok_or("invalid plugin registry")?;
        if sources.remove(first).is_none() { return Err(format!("Marketplace source not found: {first}")); }
        let file = save_plugin_registry(context.cwd, &registry)?;
        return Ok(format!("Removed marketplace source {} from {}. Installed plugin records were kept; uninstall them explicitly if desired.", first, file.display()));
    }
    if verb == "list" {
        let mut sources = sorted_object_values(&registry["sources"]);
        sources.sort_by(|a, b| a.get("name").and_then(Value::as_str).unwrap_or("").cmp(b.get("name").and_then(Value::as_str).unwrap_or("")));
        return Ok(if sources.is_empty() { "No marketplace sources configured. Add one with /marketplace add <source>.".into() } else { ["Marketplace sources:".into()].into_iter().chain(sources.iter().map(format_plugin_source)).collect::<Vec<_>>().join("\n") });
    }
    if verb == "installed" {
        let mut installed = merged_installed_values(context.cwd);
        installed.sort_by(|a, b| a.get("id").and_then(Value::as_str).unwrap_or("").cmp(b.get("id").and_then(Value::as_str).unwrap_or("")));
        return Ok(if installed.is_empty() { "No plugins installed.".into() } else { ["Installed plugins:".into()].into_iter().chain(installed.iter().map(format_plugin)).collect::<Vec<_>>().join("\n") });
    }
    if verb == "uninstall" {
        if first.is_empty() { return Err("Usage: /marketplace uninstall <name@marketplace>".into()); }
        let mut removed_from = None;
        for dir in [context.cwd.to_path_buf(), dirs_home()] {
            let mut scoped = plugin_registry(&dir);
            let removed = scoped.get_mut("installed").and_then(Value::as_object_mut).and_then(|m| m.remove(first));
            if removed.is_some() {
                let file = save_plugin_registry(&dir, &scoped)?;
                removed_from = Some(file);
            }
        }
        match removed_from {
            Some(file) => return Ok(format!("Uninstalled plugin {} from {}.", first, file.display())),
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
        if sources.is_empty() { return Ok("No marketplace sources configured. Add one with /marketplace add <source>.".into()); }
        let mut lines = vec!["Available plugins:".to_string()];
        let mut any = false;
        let mut errors = Vec::new();
        for (name, source) in sources {
            let cache = marketplace_cache_dir(&name);
            if !cache.exists() {
                if let Err(error) = fetch_marketplace(context.cwd, &name, &source) { errors.push(format!("- {name}: {error}")); continue; }
            }
            let catalog = match read_marketplace_catalog(&cache) { Ok(catalog) => catalog, Err(error) => { errors.push(format!("- {name}: {error}")); continue; } };
            let mut plugins = catalog_plugins(&catalog);
            plugins.sort_by(|a, b| a.get("name").and_then(Value::as_str).unwrap_or("").cmp(b.get("name").and_then(Value::as_str).unwrap_or("")));
            for plugin in plugins {
                let pname = plugin.get("name").and_then(Value::as_str).unwrap_or("-");
                let desc = plugin.get("description").and_then(Value::as_str).unwrap_or("");
                lines.push(format!("{pname}@{name}\t{desc}"));
                any = true;
            }
        }
        if !any { lines.push("- none".into()); }
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
        if sources.is_empty() { return Ok("No marketplace sources configured. Add one with /marketplace add <source>.".into()); }
        let mut updated = Vec::new();
        let mut counts = Vec::new();
        let mut errors = Vec::new();
        for (name, source) in sources {
            match fetch_marketplace(context.cwd, &name, &source).and_then(|cache| read_marketplace_catalog(&cache)) {
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
        let mut out = vec![format!("Updated {} marketplace source(s), {total_plugins} plugin(s) available.", updated.len())];
        out.extend(errors);
        return Ok(out.join("\n"));
    }
    if verb == "install" {
        let (force, scope, targets) = parse_marketplace_flags(rest);
        let scope = normalize_scope(scope)?;
        if targets.is_empty() { return Err("Usage: /marketplace install [--force] [--scope user|project] <name@marketplace>".into()); }
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
                    if let Some(id) = entry.get("id").and_then(Value::as_str) { ids.push((id.to_string(), scope.clone())); }
                }
            }
        } else {
            let scope = normalize_scope(scope)?;
            for target in targets { ids.push((target, scope.clone())); }
        }
        if ids.is_empty() { return Ok("No installed plugins to upgrade.".into()); }
        let mut lines = Vec::new();
        for (id, scope) in ids {
            let (plugin, mkt) = split_plugin_id(&id)?;
            lines.push(install_one(context.cwd, &mkt, &plugin, &scope, true)?);
        }
        return Ok(lines.join("\n"));
    }
    Err("Usage: /marketplace add <source> | remove <name> | list | update [name] | discover [marketplace] | install <name@marketplace> | upgrade [name@marketplace] | installed | uninstall <name@marketplace> | help".into())
}
