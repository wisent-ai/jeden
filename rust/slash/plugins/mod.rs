//! Installed plugins and extensions: what is there, what is enabled, and the
//! rows an operator manages them from.

use serde_json::{json, Value};
use std::env;
use std::path::{Path, PathBuf};

use crate::slash::common::{dirs_home, now_text, split_args};
use crate::slash::SlashContext;
use crate::tools;
use crate::tui::{PickerItem, PickerSpec};

pub(crate) mod discovery;
pub(crate) mod fetch;
pub(crate) mod marketplace;
pub(crate) mod ops;
pub(crate) mod registry;
pub(crate) use crate::marketplace as production;

use discovery::{
    configured_extension_paths, discover_custom_tool_files, discover_extension_module_files,
    native_extension_roots,
};
use ops::{
    installed_entries_for_scope, merged_installed_values, normalize_scope, parse_marketplace_flags,
    registry_scope_dir,
};
use registry::{format_plugin, plugin_registry, save_plugin_registry, sorted_object_values};

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

pub(crate) fn extensions_picker(context: &SlashContext<'_>) -> PickerSpec {
    let mut items = plugin_management_items(context.cwd);
    for (scope, dir) in [
        ("project", context.cwd.to_path_buf()),
        ("user", plugins_home()),
    ] {
        let registry = plugin_registry(&dir);
        for source in sorted_object_values(&registry["sources"]) {
            let Some(name) = source.get("name").and_then(Value::as_str) else {
                continue;
            };
            let enabled = source.get("enabled").and_then(Value::as_bool) != Some(false);
            let location = source
                .get("source")
                .and_then(Value::as_str)
                .unwrap_or("unknown source");
            items.push(
                PickerItem::action(
                    format!("Discover extensions from {name}"),
                    format!("/marketplace discover {name}"),
                )
                .detail(format!(
                    "{scope} marketplace · {location} · {}",
                    if enabled { "enabled" } else { "disabled" }
                ))
                .badge("SOURCE"),
            );
        }
    }
    for path in native_extension_roots(context.cwd)
        .into_iter()
        .flat_map(|root| discover_extension_module_files(&root))
    {
        items.push(
            PickerItem::action(format!("Native extension {}", path.display()), "")
                .detail("Discovered extension module · managed on disk")
                .badge("NATIVE")
                .disabled(true),
        );
    }
    for path in configured_extension_paths(context.cwd)
        .into_iter()
        .flat_map(|path| discover_extension_module_files(&path))
    {
        items.push(
            PickerItem::action(format!("Configured extension {}", path.display()), "")
                .detail("Configured extension module · managed in configuration")
                .badge("CONFIG")
                .disabled(true),
        );
    }
    if items.is_empty() {
        items.push(empty_picker_item("No extensions or plugins found"));
    }
    PickerSpec::new("Extensions and plugins", items)
}

pub(crate) fn reload_plugins_picker(context: &SlashContext<'_>) -> PickerSpec {
    let custom_tools = discover_custom_tool_files(context.cwd);
    let visible_tools = tools::list_tools(context.cwd);
    PickerSpec::new(
        "Reload plugins",
        vec![
            PickerItem::action("Reload plugin and tool registry", "/reload-plugins run")
                .detail(format!(
                    "{} custom tool file(s) · {} visible tool definition(s)",
                    custom_tools.len(),
                    visible_tools.len()
                ))
                .badge("RELOAD"),
        ],
    )
}

pub(crate) fn handle_extensions(context: &SlashContext<'_>) -> Result<String, String> {
    crate::hooks::extension_status(context.cwd)
}



pub(crate) fn handle_reload_plugins(context: &SlashContext<'_>) -> Result<String, String> {
    let report = crate::hooks::reload_extensions(context.cwd)?;
    Ok(format!(
        "Plugin registry reloaded live at generation {}.\nActive extensions: {}\nUnhealthy extensions: {}\nTools: {}\nCommands: {}\nHooks: {}\nCapabilities: {}",
        report.generation,
        report.active_extensions,
        report.unhealthy_extensions,
        report.tools,
        report.commands,
        report.hooks,
        report.capabilities,
    ))
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
    if verb == "enable" || verb == "disable" {
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
        let mut registry = plugin_registry(&reg_dir);
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
        plugin_obj.insert("enabled".into(), json!(verb == "enable"));
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
