//! The rows shown for the external tool servers this workspace can talk to.
//!
//! Split out of `slash/commands/connections/mcp.rs`, which had grown past the
//! module line cap.

use crate::mcp;
use crate::slash::common::{dirs_home, read_json_value};
use crate::slash::SlashContext;
use crate::tui::{PickerItem, PickerSpec};
use serde_json::Value;

pub(crate) fn mcp_picker(context: &SlashContext<'_>) -> PickerSpec {
    let user = read_json_value(&dirs_home().join(".jeden/mcp.json"));
    let project = read_json_value(&context.cwd.join(".jeden/mcp.json"));
    let merged = mcp::load_config(context.cwd);
    let disabled = merged
        .get("disabledServers")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .collect::<std::collections::BTreeSet<_>>();
    let mut items = vec![
        PickerItem::action("Show MCP server status", "/mcp status")
            .detail("Effective user and project configuration")
            .badge("status"),
        PickerItem::action("Reload and probe enabled servers", "/mcp reload")
            .detail("Re-read config and perform real tools probes")
            .badge("reconnect"),
    ];
    if let Some(servers) = merged.get("mcpServers").and_then(Value::as_object) {
        for (name, server) in std::collections::BTreeMap::from_iter(servers.iter()) {
            let in_scope = |config: &Value| {
                config
                    .get("mcpServers")
                    .and_then(Value::as_object)
                    .map(|servers| servers.contains_key(name))
                    .unwrap_or(false)
            };
            let scope = if in_scope(&project) {
                "project"
            } else if in_scope(&user) {
                "user"
            } else {
                "merged"
            };
            let status = if disabled.contains(name.as_str()) {
                "disabled"
            } else {
                "enabled"
            };
            let transport = server.get("type").and_then(Value::as_str).unwrap_or(
                if server.get("url").is_some() {
                    "http"
                } else {
                    "stdio"
                },
            );
            let badge = format!("{status} · {scope}");
            let disabled_in = |config: &Value| {
                config
                    .get("disabledServers")
                    .and_then(Value::as_array)
                    .map(|values| values.iter().any(|value| value.as_str() == Some(name)))
                    .unwrap_or(false)
            };
            let toggle_scope = if status == "disabled" && disabled_in(&project) {
                "project"
            } else if status == "disabled" && disabled_in(&user) {
                "user"
            } else {
                scope
            };
            let scope_flag = if toggle_scope == "project" {
                " --scope project"
            } else {
                " --scope user"
            };
            let source_scope_flag = if scope == "project" {
                " --scope project"
            } else {
                " --scope user"
            };
            let toggle = if status == "enabled" {
                "disable"
            } else {
                "enable"
            };
            let toggle_label = if status == "enabled" {
                "Disable"
            } else {
                "Enable"
            };
            items.push(
                PickerItem::action(
                    format!("{toggle_label} {name}"),
                    format!("/mcp {toggle}{scope_flag} {name}"),
                )
                .detail(format!("{transport} transport; defined in {scope} scope"))
                .badge(badge.clone()),
            );
            items.push(
                PickerItem::action(
                    format!("Reconnect {name}"),
                    format!("/mcp reconnect {name}"),
                )
                .detail("Re-spawn the server and perform a tools probe")
                .badge(badge.clone())
                .disabled(status != "enabled"),
            );
            for (label, verb) in [
                ("Tools", "tools"),
                ("Resources", "resources"),
                ("Prompts", "prompts"),
                ("Notifications", "notifications"),
                ("Test", "test"),
            ] {
                items.push(
                    PickerItem::action(format!("{label}: {name}"), format!("/mcp {verb} {name}"))
                        .detail(format!(
                            "Query {label} from the configured {transport} server"
                        ))
                        .badge(badge.clone())
                        .disabled(status != "enabled"),
                );
            }
            if matches!(scope, "user" | "project") {
                items.push(
                    PickerItem::action(
                        format!("Remove {name}"),
                        format!("/mcp remove{source_scope_flag} {name}"),
                    )
                    .detail(format!("Delete the definition from {scope} scope"))
                    .badge("DESTRUCTIVE"),
                );
            }
        }
    }
    let add = PickerItem::action("Add an MCP server", "/mcp add --scope project ")
        .detail("Edit name, transport, command, and arguments before submitting")
        .badge("INPUT")
        .prefill();
    items.push(add);
    PickerSpec::new("MCP servers", items)
}
