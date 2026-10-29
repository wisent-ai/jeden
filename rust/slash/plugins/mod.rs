//! Installed plugins and extensions: what is there, what is enabled, and the
//! rows an operator manages them from.

use serde::Deserialize;
use serde_json::{json, Value};
use std::env;
use std::path::{Path, PathBuf};

use crate::slash::common::{dirs_home, now_text, split_args};
use crate::slash::SlashContext;
use crate::tui::{PickerItem, PickerSpec};

pub(crate) mod discovery;
mod extensions;
pub(crate) mod fetch;
pub(crate) mod marketplace;
pub(crate) mod ops;
pub(crate) mod registry;
pub(crate) use crate::marketplace as production;
pub(crate) use extensions::{
    extensions_picker, handle_extensions, handle_reload_plugins, reload_plugins_picker,
};

use ops::{
    installed_entries_for_scope, merged_installed_values, normalize_scope, parse_marketplace_flags,
    registry_scope_dir,
};
use registry::{format_plugin, plugin_registry, save_plugin_registry};

/// The `/plugins` verbs that switch an installed plugin on or off.
#[derive(Deserialize)]
#[serde(rename_all = "lowercase")]
enum Toggle {
    Enable,
    Disable,
}

/// Plugin cache home. Honors `JEDEN_PLUGINS_HOME` (used to keep tests hermetic);
/// defaults to `~`.
pub(crate) fn plugins_home() -> PathBuf {
    match env::var_os("JEDEN_PLUGINS_HOME") {
        Some(home) => PathBuf::from(home),
        None => dirs_home(),
    }
}

pub(crate) fn marketplace_cache_root() -> PathBuf {
    plugins_home().join(".jeden/plugins/cache/marketplaces")
}

pub(crate) fn marketplace_cache_dir(name: &str) -> PathBuf {
    marketplace_cache_root().join(name)
}

fn empty_picker_item(message: &str) -> PickerItem {
    PickerItem::action(message, "")
        .detail("Nothing to manage yet")
        .disabled(true)
}

/// Every installed plugin of each scope, by id then scope; a scope whose
/// package registry cannot be read is refused, never listed as empty.
fn scoped_installed_plugins(cwd: &Path) -> Result<Vec<(String, Value)>, String> {
    let mut plugins = Vec::new();
    for (scope, dir) in [("project", cwd.to_path_buf()), ("user", plugins_home())] {
        for mut entry in installed_entries_for_scope(&dir)? {
            if entry.get("scope").and_then(Value::as_str).is_none() {
                entry["scope"] = json!(scope);
            }
            plugins.push((scope.to_string(), entry));
        }
    }
    plugins.sort_by(|(scope_a, a), (scope_b, b)| {
        a.get("id")
            .and_then(Value::as_str)
            .cmp(&b.get("id").and_then(Value::as_str))
            .then_with(|| scope_a.cmp(scope_b))
    });
    Ok(plugins)
}

/// One field of an installed record as its row shows it, or that the record
/// states none.
fn stated<'a>(plugin: &'a Value, key: &str, absent: &'a str) -> &'a str {
    match plugin.get(key).and_then(Value::as_str) {
        Some(value) => value,
        None => absent,
    }
}

fn plugin_management_items(cwd: &Path) -> Result<Vec<PickerItem>, String> {
    let mut items = Vec::new();
    for (scope, plugin) in scoped_installed_plugins(cwd)? {
        let Some(id) = plugin.get("id").and_then(Value::as_str) else {
            continue;
        };
        let enabled = plugin.get("enabled").and_then(Value::as_bool) != Some(false);
        let version = stated(&plugin, "version", "no version recorded");
        let source = stated(&plugin, "source", "no source recorded");
        let state = if enabled { "enabled" } else { "disabled" };
        let detail = format!("{scope} scope · {version} · {source} · {state}");
        let toggle = if enabled { "disable" } else { "enable" };
        items.push(
            PickerItem::action(
                format!("{} {id}", if enabled { "Disable" } else { "Enable" }),
                format!("/plugins {toggle} --scope {scope} {id}"),
            )
            .detail(&detail)
            .badge(if enabled { "ENABLED" } else { "DISABLED" }),
        );
        items.push(
            PickerItem::action(
                format!("Upgrade {id} ({scope})"),
                format!("/marketplace upgrade --scope {scope} {id}"),
            )
            .detail(&detail)
            .badge("UPGRADE"),
        );
        items.push(
            PickerItem::action(
                format!("Uninstall {id} (all scopes)"),
                format!("/marketplace uninstall {id}"),
            )
            .detail(format!("installed in {scope} scope · {version} · {source}"))
            .badge("DESTRUCTIVE"),
        );
    }
    Ok(items)
}

pub(crate) fn plugins_picker(context: &SlashContext<'_>) -> PickerSpec {
    let items = match plugin_management_items(context.cwd) {
        Ok(items) if items.is_empty() => vec![empty_picker_item("No plugins installed")],
        Ok(items) => items,
        // The registry that could not be read is one disabled error row, as
        // the other plugin views show it, never "No plugins installed".
        Err(cause) => vec![PickerItem::action(&cause, "")
            .detail("error")
            .disabled(true)],
    };
    PickerSpec::new("Manage plugins", items)
}

pub(crate) fn handle_plugins(args: &str, context: &SlashContext<'_>) -> Result<String, String> {
    let argv = split_args(args);
    // A bare /plugins lists, as its usage line reads.
    let (verb, rest) = match argv.split_first() {
        Some((verb, rest)) => (verb.as_str(), rest),
        None => ("list", &argv[..]),
    };
    if verb == "list" {
        let mut installed = merged_installed_values(context.cwd)?;
        installed.sort_by(|a, b| {
            a.get("id")
                .and_then(Value::as_str)
                .cmp(&b.get("id").and_then(Value::as_str))
        });
        return Ok(if installed.is_empty() {
            "No plugins installed. Use /marketplace discover and /marketplace install <name@marketplace>.".into()
        } else {
            ["Installed plugins:".into()]
                .into_iter()
                .chain(installed.iter().map(format_plugin))
                .collect::<Vec<_>>()
                .join("\n")
        });
    }
    if let Ok(toggle) = serde_json::from_value::<Toggle>(Value::from(verb)) {
        let (_force, scope, targets) = parse_marketplace_flags(rest)?;
        let scope = normalize_scope(scope)?;
        let Some(target) = targets
            .first()
            .map(String::as_str)
            .filter(|target| !target.is_empty())
        else {
            return Err(format!(
                "Usage: /plugins {verb} [--scope user|project] <name@marketplace>"
            ));
        };
        let reg_dir = registry_scope_dir(context.cwd, &scope);
        let active_here = installed_entries_for_scope(&reg_dir)?
            .iter()
            .any(|entry| entry["id"] == target);
        let mut registry = plugin_registry(&reg_dir)?;
        let installed = registry
            .get_mut("installed")
            .and_then(Value::as_object_mut)
            .ok_or("invalid plugin registry")?;
        // A package the marketplace installed has no toggle record until it
        // is first enabled or disabled; one installed nowhere is refused.
        if !installed.contains_key(target) {
            if !active_here {
                return Err(format!(
                    "Installed plugin not found in {scope} scope: {target}"
                ));
            }
            installed.insert(target.to_string(), json!({ "id": target }));
        }
        let plugin = installed
            .get_mut(target)
            .ok_or_else(|| format!("Installed plugin not found in {scope} scope: {target}"))?;
        let Some(plugin_obj) = plugin.as_object_mut() else {
            return Err(format!(
                "the installed record of {target} in {scope} scope ({}) is not an object, so it is not changed",
                reg_dir.display()
            ));
        };
        plugin_obj.insert("enabled".into(), json!(matches!(toggle, Toggle::Enable)));
        plugin_obj.insert("scope".into(), json!(scope.clone()));
        plugin_obj.insert("updatedAt".into(), json!(now_text()?));
        let file = save_plugin_registry(&reg_dir, &registry)?;
        return Ok(format!(
            "{} plugin {} in {} scope ({}).",
            if verb == "enable" {
                "Enabled"
            } else {
                "Disabled"
            },
            target,
            scope,
            file.display()
        ));
    }
    Err("Usage: /plugins list | enable [--scope user|project] <name@marketplace> | disable [--scope user|project] <name@marketplace>".into())
}
