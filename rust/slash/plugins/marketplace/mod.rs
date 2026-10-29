//! Marketplace sources: where plugins come from, and the commands that add,
//! remove and list them.

use serde_json::{json, Value};
use std::fs;

use super::fetch::{fetch_marketplace, read_marketplace_catalog};
use super::marketplace_cache_dir;

use super::ops::{
    normalize_scope, parse_marketplace_flags, production_service, registry_scope_dir,
};
use super::registry::{
    format_plugin_source, plugin_registry, save_plugin_registry, sorted_object_values,
};
use crate::slash::common::split_args;
use crate::slash::validate::valid_marketplace_name;
use crate::slash::SlashContext;

mod catalog;
mod picker;

use crate::slash::plugins::plugins_home;
use crate::slash::support::common::now_text;
use catalog::handle_catalog;
pub(crate) use picker::marketplace_picker;

/// Sanitize a string into a marketplace/version cache component: keep
/// alphanumerics and `.`/`_`/`-`, collapse other runs to a single `-`, trim
/// leading/trailing `-`. Returns an empty string when nothing survives (callers
/// treat an empty result as invalid rather than substituting a default).
pub(crate) fn sanitize_marketplace_name(value: &str) -> String {
    let mut out = String::new();
    for ch in value.trim().chars() {
        if ch.is_ascii_alphanumeric() || ch == '.' || ch == '_' || ch == '-' {
            out.push(ch);
        } else if matches!(ch, '@' | '/' | '\\') || !out.ends_with('-') {
            // An explicit separator earns its own dash even beside one already
            // written; every other rejected character just joins the run.
            out.push('-');
        }
    }
    out.trim_matches('-').to_string()
}

fn marketplace_source_name(source: &str) -> String {
    let text = source.trim().trim_end_matches('/');
    let tail = match text.rsplit_once('/') {
        Some((_, tail)) => tail,
        None => text,
    };
    sanitize_marketplace_name(if tail.is_empty() { text } else { tail })
}

fn marketplace_source_type(source: &str) -> &'static str {
    let text = source.trim().to_ascii_lowercase();
    if text.starts_with("http://") || text.starts_with("https://") {
        "url"
    } else if text.starts_with("ssh://")
        || text.starts_with("git+ssh://")
        || text.starts_with("git@")
    {
        "git"
    } else {
        "local"
    }
}

pub(crate) fn handle_marketplace(args: &str, context: &SlashContext<'_>) -> Result<String, String> {
    let argv = split_args(args);
    // A bare /marketplace is its help, as the usage line reads.
    let (verb, rest) = match argv.split_first() {
        Some((verb, rest)) => (verb.as_str(), rest),
        None => ("help", &argv[..]),
    };
    let first = match rest.first() {
        Some(first) => first.as_str(),
        None => "",
    };
    let mut registry = plugin_registry(context.cwd)?;
    if verb == "help" {
        return Ok("Usage: /marketplace add <signed-source> | dev-link [--scope user|project] <id> <local-path> [entrypoint] | remove <name> | list | update [name] | discover [marketplace] | install [--force] [--scope user|project] <name@marketplace> | upgrade [--scope user|project] [name@marketplace] | installed | uninstall <name@marketplace>.".into());
    }
    if verb == "dev-link" {
        let (_, requested_scope, positional) = parse_marketplace_flags(rest)?;
        // https://docs.npmjs.com/cli/configuring-npm/package-json#main
        // "If main is not set, it defaults to index.js in the package's root folder."
        let (id, local, entrypoint) = match positional.as_slice() {
            [id, local] => (id, local, "index.js"),
            [id, local, entrypoint] => (id, local, entrypoint.as_str()),
            _ => {
                return Err("Usage: /marketplace dev-link [--scope user|project] <id> <local-path> [entrypoint]".into())
            }
        };
        let scope = normalize_scope(requested_scope)?;
        let scope_dir = registry_scope_dir(context.cwd, &scope);
        let raw_path = std::path::PathBuf::from(local);
        let path = if raw_path.is_absolute() {
            raw_path
        } else {
            context.cwd.join(raw_path)
        };
        let record = production_service(&scope_dir).dev_link(id, &path, entrypoint)?;
        return Ok(format!("Registered explicit untrusted dev-link {} at {} as installed/inactive; marketplace verification was not bypassed.", record.id, record.path.display()));
    }
    if verb == "add" {
        let source = rest.to_vec().join(" ").trim().to_string();
        if source.is_empty() {
            return Err("Usage: /marketplace add <source>".into());
        }
        let provisional = marketplace_source_name(&source);
        if !valid_marketplace_name(&provisional) {
            return Err(format!(
                "Cannot derive a valid marketplace name from '{source}'."
            ));
        }
        // Jeden keys a marketplace by its catalog `name`. Fetch and read the catalog
        // authoritatively so errors surface, then rekey the cache to that name.
        let cache = fetch_marketplace(context.cwd, &provisional, &source)?;
        let catalog = read_marketplace_catalog(&cache)?;
        let cn = match catalog
            .pointer("/catalog/catalogId")
            .or_else(|| catalog.get("name"))
            .and_then(Value::as_str)
        {
            Some(named) => named.trim().to_string(),
            None => {
                return Err(format!(
                    "Marketplace catalog at '{source}' names neither catalog.catalogId nor name."
                ))
            }
        };
        let name = if valid_marketplace_name(&cn) {
            if cn != provisional {
                let to = marketplace_cache_dir(&cn);
                if let Some(parent) = to.parent() {
                    fs::create_dir_all(parent).map_err(|e| {
                        format!(
                            "failed to make the marketplace cache folder {}: {e}",
                            parent.display()
                        )
                    })?;
                }
                match fs::remove_dir_all(&to) {
                    Ok(()) => {}
                    Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                    Err(e) => {
                        return Err(format!(
                            "failed to clear the earlier cache of marketplace '{cn}' at {}: {e}",
                            to.display()
                        ))
                    }
                }
                fs::rename(&cache, &to)
                    .map_err(|e| format!("failed to key marketplace cache to '{cn}': {e}"))?;
            }
            cn
        } else {
            return Err(format!(
                "Marketplace catalog at '{source}' has an invalid or missing name."
            ));
        };
        let existing = registry
            .get("sources")
            .and_then(Value::as_object)
            .and_then(|sources| sources.get(&name))
            .cloned();
        let now = now_text()?;
        // A source added before keeps the time it was first added and the
        // plugins it listed; a new one lists none yet.
        let added_at = match existing.as_ref().and_then(|earlier| earlier.get("addedAt")) {
            Some(first) => first.clone(),
            None => json!(now),
        };
        let listed = match existing
            .as_ref()
            .and_then(|earlier| earlier.get("plugins"))
            .filter(|value| value.is_array())
        {
            Some(plugins) => plugins.clone(),
            None => json!([]),
        };
        registry
            .get_mut("sources")
            .and_then(Value::as_object_mut)
            .ok_or("invalid plugin registry")?
            .insert(
                name.clone(),
                json!({
                    "name": name,
                    "source": source.clone(),
                    "type": marketplace_source_type(&source),
                    "enabled": true,
                    "addedAt": added_at,
                    "updatedAt": now,
                    "plugins": listed,
                }),
            );
        let file = save_plugin_registry(context.cwd, &registry)?;
        return Ok(format!(
            "Added marketplace source {} ({}) in {}.",
            name,
            source,
            file.display()
        ));
    }
    if verb == "remove" {
        if first.is_empty() {
            return Err("Usage: /marketplace remove <name>".into());
        }
        let sources = registry
            .get_mut("sources")
            .and_then(Value::as_object_mut)
            .ok_or("invalid plugin registry")?;
        if sources.remove(first).is_none() {
            return Err(format!("Marketplace source not found: {first}"));
        }
        let file = save_plugin_registry(context.cwd, &registry)?;
        return Ok(format!("Removed marketplace source {} from {}. Installed plugin records were kept; uninstall them explicitly if desired.", first, file.display()));
    }
    if verb == "list" {
        let mut sources = sorted_object_values(&registry["sources"])?;
        sources.sort_by(|a, b| {
            a.get("name")
                .and_then(Value::as_str)
                .cmp(&b.get("name").and_then(Value::as_str))
        });
        return Ok(if sources.is_empty() {
            "No marketplace sources configured. Add one with /marketplace add <source>.".into()
        } else {
            ["Marketplace sources:".into()]
                .into_iter()
                .chain(sources.iter().map(format_plugin_source))
                .collect::<Vec<_>>()
                .join("\n")
        });
    }
    handle_catalog(verb, rest, first, context)
}
