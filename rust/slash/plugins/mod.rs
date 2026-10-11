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
    env::var_os("JEDEN_PLUGINS_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(dirs_home)
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

fn scoped_installed_plugins(cwd: &Path) -> Vec<(String, Value)> {
    let mut plugins = Vec::new();
    for (scope, dir) in [("project", cwd.to_path_buf()), ("user", plugins_home())] {
        for mut entry in installed_entries_for_scope(&dir) {
            if entry.get("scope").and_then(Value::as_str).is_none() {
                entry["scope"] = json!(scope);
            }
            plugins.push((scope.to_string(), entry));
        }
    }
    plugins.sort_by(|(scope_a, a), (scope_b, b)| {
        a.get("id")
            .and_then(Value::as_str)
            .unwrap_or("")
            .cmp(b.get("id").and_then(Value::as_str).unwrap_or(""))
            .then_with(|| scope_a.cmp(scope_b))
    });
    plugins
}

fn plugin_management_items(cwd: &Path) -> Vec<PickerItem> {
    let mut items = Vec::new();
    for (scope, plugin) in scoped_installed_plugins(cwd) {
        let Some(id) = plugin.get("id").and_then(Value::as_str) else {
            continue;
        };
        let enabled = plugin.get("enabled").and_then(Value::as_bool) != Some(false);
        let version = plugin
            .get("version")
            .and_then(Value::as_str)
            .unwrap_or("unknown version");
        let source = plugin
            .get("source")
            .and_then(Value::as_str)
            .unwrap_or("unknown source");
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
    items
}

pub(crate) fn plugins_picker(context: &SlashContext<'_>) -> PickerSpec {
    let mut items = plugin_management_items(context.cwd);
    if items.is_empty() {
        items.push(empty_picker_item("No plugins installed"));
    }
    PickerSpec::new("Manage plugins", items)
}

pub(crate) fn handle_plugins(args: &str, context: &SlashContext<'_>) -> Result<String, String> {
    let argv = split_args(args);
    let verb = argv.first().map(String::as_str).unwrap_or("list");
    if verb == "list" {
        let mut installed = merged_installed_values(context.cwd);
        installed.sort_by(|a, b| {
            a.get("id")
                .and_then(Value::as_str)
                .unwrap_or("")
                .cmp(b.get("id").and_then(Value::as_str).unwrap_or(""))
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
        let rest = argv.split_first().map(|(_, r)| r).unwrap_or(&[]);
        let (_force, scope, targets) = parse_marketplace_flags(rest);
        let scope = normalize_scope(scope)?;
        let target = targets.first().map(String::as_str).unwrap_or("");
        if target.is_empty() {
            return Err(format!(
                "Usage: /plugins {verb} [--scope user|project] <name@marketplace>"
            ));
        }
        let reg_dir = registry_scope_dir(context.cwd, &scope);
        let mut registry = plugin_registry(&reg_dir)?;
        let installed = registry
            .get_mut("installed")
            .and_then(Value::as_object_mut)
            .ok_or("invalid plugin registry")?;
        let plugin = installed
            .get_mut(target)
            .ok_or_else(|| format!("Installed plugin not found in {scope} scope: {target}"))?;
        if !plugin.is_object() {
            *plugin = json!({});
        }
        let plugin_obj = plugin.as_object_mut().expect("plugin object");
        plugin_obj.insert("enabled".into(), json!(matches!(toggle, Toggle::Enable)));
        plugin_obj.insert("scope".into(), json!(scope.clone()));
        plugin_obj.insert("updatedAt".into(), json!(now_text()));
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
