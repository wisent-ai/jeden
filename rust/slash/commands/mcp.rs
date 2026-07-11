use serde_json::{json, Value};
use std::path::{Path, PathBuf};

use crate::mcp;
use crate::slash::common::{dirs_home, read_json_value, split_args, split_head, write_json_value};
use crate::slash::plugins::ops::{normalize_scope, parse_marketplace_flags};
use crate::slash::SlashContext;
use crate::tui::{PickerItem, PickerSpec};

fn mcp_scope_file(cwd: &Path, scope: &str) -> Result<PathBuf, String> {
    match scope {
        "user" => Ok(dirs_home().join(".jeden/mcp.json")),
        "project" => Ok(cwd.join(".jeden/mcp.json")),
        other => Err(format!("Invalid scope: {other}. Use user or project.")),
    }
}

fn valid_mcp_server_name(name: &str) -> bool {
    !name.is_empty()
        && name
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '_' | '-' | '.'))
}

fn mcp_write_file(path: &Path, mut config: Value) -> Result<PathBuf, String> {
    if !config.is_object() {
        config = json!({});
    }
    let obj = config.as_object_mut().expect("object");
    obj.entry("mcpServers").or_insert_with(|| json!({}));
    obj.entry("disabledServers").or_insert_with(|| json!([]));
    write_json_value(path, &config)?;
    Ok(path.to_path_buf())
}

fn active_mcp_server_names(cwd: &Path) -> Vec<String> {
    let config = mcp::load_config(cwd);
    let disabled = config
        .get("disabledServers")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .collect::<std::collections::BTreeSet<_>>();
    let mut names = config
        .get("mcpServers")
        .and_then(Value::as_object)
        .map(|servers| {
            servers
                .keys()
                .filter(|name| !disabled.contains(name.as_str()))
                .cloned()
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    names.sort();
    names
}

fn format_mcp_list(cwd: &Path) -> String {
    let user = read_json_value(&dirs_home().join(".jeden/mcp.json"));
    let project = read_json_value(&cwd.join(".jeden/mcp.json"));
    let merged = mcp::load_config(cwd);
    let disabled = merged
        .get("disabledServers")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .collect::<std::collections::BTreeSet<_>>();
    let Some(servers) = merged.get("mcpServers").and_then(Value::as_object) else {
        return "No MCP servers configured.".into();
    };
    if servers.is_empty() {
        return "No MCP servers configured.".into();
    }
    let mut lines = Vec::new();
    for (name, server) in std::collections::BTreeMap::from_iter(servers.iter()) {
        let source = if project
            .get("mcpServers")
            .and_then(Value::as_object)
            .map(|m| m.contains_key(name))
            .unwrap_or(false)
        {
            "project"
        } else if user
            .get("mcpServers")
            .and_then(Value::as_object)
            .map(|m| m.contains_key(name))
            .unwrap_or(false)
        {
            "user"
        } else {
            "merged"
        };
        let status = if disabled.contains(name.as_str())
            || server.get("enabled").and_then(Value::as_bool) == Some(false)
        {
            "disabled"
        } else {
            "enabled"
        };
        let transport =
            server
                .get("type")
                .and_then(Value::as_str)
                .unwrap_or(if server.get("url").is_some() {
                    "http"
                } else {
                    "stdio"
                });
        lines.push(format!("{name} [{status}, {source}, {transport}]"));
    }
    lines.join("\n")
}

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

fn head_rest(parts: &[String]) -> (&str, &[String]) {
    match parts.split_first() {
        Some((first, rest)) => (first.as_str(), rest),
        None => ("", &[]),
    }
}

pub(crate) fn handle_mcp(args: &str, context: &SlashContext<'_>) -> Result<String, String> {
    let (verb, rest) = split_head(args);
    let verb = if verb.is_empty() { "list" } else { verb };
    match verb {
        "list" => Ok(format_mcp_list(context.cwd)),
        "status" => serde_json::to_string_pretty(&mcp::manager_status(context.cwd)?).map_err(|error| error.to_string()),
        "add" => {
            let (_force, scope, parts) = parse_marketplace_flags(&split_args(rest));
            let scope = normalize_scope(scope)?;
            let (name, after_name) = head_rest(&parts);
            if !valid_mcp_server_name(name) { return Err("Usage: /mcp add [--scope user|project] <name> [stdio <command> [args...] | http <url>]".into()); }
            let (transport_or_command, after_transport) = head_rest(after_name);
            if transport_or_command.is_empty() { return Err("Usage: /mcp add [--scope user|project] <name> [stdio <command> [args...] | http <url>]".into()); }
            let server = match transport_or_command {
                "http" | "streamable-http" => {
                    let (url, extra) = head_rest(after_transport);
                    if url.is_empty() || !extra.is_empty() || !(url.starts_with("http://") || url.starts_with("https://")) {
                        return Err(format!("Usage: /mcp add {name} http <http(s)-url>"));
                    }
                    json!({"type": "streamable-http", "url": url})
                }
                "sse" => return Err("Legacy SSE transport is not supported; use streamable HTTP.".into()),
                "stdio" => {
                    let (command, after_command) = head_rest(after_transport);
                    if command.is_empty() { return Err(format!("Usage: /mcp add {name} stdio <command> [args...]")); }
                    json!({"type": "stdio", "command": command, "args": after_command.to_vec()})
                }
                command => json!({"type": "stdio", "command": command, "args": after_transport.to_vec()}),
            };
            let path = mcp_scope_file(context.cwd, &scope)?;
            let mut config = read_json_value(&path);
            if !config.is_object() { config = json!({}); }
            config.as_object_mut().unwrap().entry("mcpServers").or_insert_with(|| json!({}));
            config["mcpServers"][name] = server;
            let path = mcp_write_file(&path, config)?;
            mcp::manager_status(context.cwd)?;
            Ok(format!("Added MCP server {name} in {scope} scope ({})", path.display()))
        },
        "remove" => {
            let (_force, scope, parts) = parse_marketplace_flags(&split_args(rest));
            let scope = normalize_scope(scope)?;
            let name = parts.first().map(String::as_str).unwrap_or("");
            if name.is_empty() { return Err("Usage: /mcp remove [--scope user|project] <name>".into()); }
            let path = mcp_scope_file(context.cwd, &scope)?;
            let mut config = read_json_value(&path);
            let removed = config.get_mut("mcpServers").and_then(Value::as_object_mut).and_then(|servers| servers.remove(name)).is_some();
            if let Some(disabled) = config.get_mut("disabledServers").and_then(Value::as_array_mut) {
                disabled.retain(|value| value.as_str() != Some(name));
            }
            let path = mcp_write_file(&path, config)?;
            mcp::manager_status(context.cwd)?;
            if removed { Ok(format!("Removed MCP server {name} from {scope} scope ({})", path.display())) } else { Err(format!("MCP server not found in {scope} scope: {name}")) }
        },
        "enable" | "disable" => {
            let (_force, scope, parts) = parse_marketplace_flags(&split_args(rest));
            let scope = normalize_scope(scope)?;
            let name = parts.first().map(String::as_str).unwrap_or("");
            if name.is_empty() { return Err(format!("Usage: /mcp {verb} [--scope user|project] <name>")); }
            let path = mcp_scope_file(context.cwd, &scope)?;
            let mut config = read_json_value(&path);
            if !config.is_object() { config = json!({}); }
            config.as_object_mut().unwrap().entry("disabledServers").or_insert_with(|| json!([]));
            let disabled = config.get_mut("disabledServers").and_then(Value::as_array_mut).ok_or("disabledServers must be an array")?;
            if verb == "disable" {
                if !disabled.iter().any(|value| value.as_str() == Some(name)) { disabled.push(json!(name)); }
            } else {
                disabled.retain(|value| value.as_str() != Some(name));
                if let Some(server) = config.get_mut("mcpServers").and_then(Value::as_object_mut).and_then(|servers| servers.get_mut(name)).and_then(Value::as_object_mut) {
                    if server.get("enabled").and_then(Value::as_bool) == Some(false) {
                        server.insert("enabled".into(), json!(true));
                    }
                }
            }
            let path = mcp_write_file(&path, config)?;
            mcp::manager_status(context.cwd)?;
            Ok(format!("{} MCP server {name} in {scope} scope ({})", if verb == "enable" { "Enabled" } else { "Disabled" }, path.display()))
        },
        "tools" | "resources" | "prompts" | "test" => {
            let (server, _) = split_head(rest);
            if server.is_empty() { return Err(format!("Usage: /mcp {} <server>", verb)); }
            let result = match verb {
                "tools" | "test" => mcp::list_tools(context.cwd, server, u64::MAX),
                "resources" => mcp::list_resources(context.cwd, server, u64::MAX),
                "prompts" => mcp::list_prompts(context.cwd, server, u64::MAX),
                _ => unreachable!(),
            }?;
            serde_json::to_string_pretty(&result).map_err(|e| e.to_string())
        },
        "notifications" => {
            let (server, _) = split_head(rest);
            if server.is_empty() { return Err("Usage: /mcp notifications <server>".into()); }
            let init = mcp::server_capabilities(context.cwd, server, u64::MAX)?;
            let capabilities = init.get("capabilities").cloned().unwrap_or_else(|| json!({}));
            Ok(format!(
                "MCP notification capabilities for {} (live persistent connection):\n{}",
                server,
                serde_json::to_string_pretty(&capabilities).map_err(|e| e.to_string())?
            ))
        },
        "reload" => {
            let status = mcp::refresh_all(context.cwd, u64::MAX)?;
            Ok(format!(
                "Reloaded MCP config and refreshed live connections:\n{}",
                serde_json::to_string_pretty(&status).map_err(|error| error.to_string())?
            ))
        }
        "reconnect" => {
            let (server, _) = split_head(rest);
            if server.is_empty() { return Err("Usage: /mcp reconnect <server>".into()); }
            let status = mcp::reconnect(context.cwd, server, u64::MAX)
                .map_err(|error| format!("Reconnect to {server} failed: {error}"))?;
            Ok(format!(
                "Reconnected MCP server {server}:\n{}",
                serde_json::to_string_pretty(&status).map_err(|error| error.to_string())?
            ))
        }
        _ => Err("Usage: /mcp list | status | add [--scope user|project] <name> [stdio <command> [args...] | http <url>] | remove [--scope user|project] <name> | enable [--scope user|project] <name> | disable [--scope user|project] <name> | tools <server> | resources <server> | prompts <server> | notifications <server> | test <server> | reload | reconnect <server>".into()),
    }
}
