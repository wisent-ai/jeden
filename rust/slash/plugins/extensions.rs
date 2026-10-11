//! `/extensions` and `/reload-plugins`: the extension modules and plugin
//! sources this workspace and user see, and the live reload of them.

use serde_json::Value;
use std::path::Path;

use super::discovery::{
    configured_extension_paths, discover_custom_tool_files, native_extension_roots,
};
use super::registry::{plugin_registry, sorted_object_values};
use super::{empty_picker_item, plugin_management_items, plugins_home};
use crate::hooks::extensions::loading::scan_modules;
use crate::slash::common::refusal_picker;
use crate::slash::SlashContext;
use crate::tools;
use crate::tui::{PickerItem, PickerSpec};

/// One disabled row per extension module under `root`. A root that cannot be
/// listed, or a package manifest that cannot be read, is one row naming it,
/// not a root that silently holds nothing.
fn discovered_rows(
    items: &mut Vec<PickerItem>,
    root: &Path,
    origin: &str,
    detail: &str,
    badge: &str,
) {
    match scan_modules(root, true) {
        Ok(paths) => {
            for path in paths {
                items.push(
                    PickerItem::action(format!("{origin} extension {}", path.display()), "")
                        .detail(detail)
                        .badge(badge)
                        .disabled(true),
                );
            }
        }
        Err(cause) => items.push(
            PickerItem::action(format!("{origin} extensions cannot be listed"), "")
                .detail(cause)
                .badge("error")
                .disabled(true),
        ),
    }
}

pub(crate) fn extensions_picker(context: &SlashContext<'_>) -> PickerSpec {
    let mut items = plugin_management_items(context.cwd);
    for (scope, dir) in [
        ("project", context.cwd.to_path_buf()),
        ("user", plugins_home()),
    ] {
        let registry = match plugin_registry(&dir) {
            Ok(registry) => registry,
            Err(cause) => {
                return refusal_picker(
                    "Extensions and plugins",
                    "Plugin registry cannot be read",
                    cause,
                )
            }
        };
        for source in sorted_object_values(&registry["sources"]) {
            let Some(name) = source.get("name").and_then(Value::as_str) else {
                continue;
            };
            let enabled = source.get("enabled").and_then(Value::as_bool) != Some(false);
            let location = match source.get("source").and_then(Value::as_str) {
                Some(location) => location.to_string(),
                None => format!("the registry records no source for {name}"),
            };
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
    for root in native_extension_roots(context.cwd) {
        discovered_rows(
            &mut items,
            &root,
            "Native",
            "Discovered extension module · managed on disk",
            "NATIVE",
        );
    }
    for path in configured_extension_paths(context.cwd) {
        discovered_rows(
            &mut items,
            &path,
            "Configured",
            "Configured extension module · managed in configuration",
            "CONFIG",
        );
    }
    if items.is_empty() {
        items.push(empty_picker_item("No extensions or plugins found"));
    }
    PickerSpec::new("Extensions and plugins", items)
}

pub(crate) fn reload_plugins_picker(context: &SlashContext<'_>) -> PickerSpec {
    let visible_tools = tools::list_tools(context.cwd);
    let detail = match discover_custom_tool_files(context.cwd) {
        Ok(custom_tools) => format!(
            "{} custom tool file(s) · {} visible tool definition(s)",
            custom_tools.len(),
            visible_tools.len()
        ),
        Err(error) => format!(
            "custom tool files unknown: {error} · {} visible tool definition(s)",
            visible_tools.len()
        ),
    };
    PickerSpec::new(
        "Reload plugins",
        vec![
            PickerItem::action("Reload plugin and tool registry", "/reload-plugins run")
                .detail(detail)
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
