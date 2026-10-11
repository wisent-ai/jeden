//! The external tool servers this workspace can talk to: declaring one,
//! enabling it, and removing it.

use serde::Deserialize;
use serde_json::{json, Value};
use std::path::{Path, PathBuf};

use crate::mcp;
use crate::slash::common::{dirs_home, split_args, split_head, write_json_value};
use crate::slash::plugins::ops::{normalize_scope, parse_marketplace_flags};
use crate::slash::SlashContext;

mod list;
mod picker;

use list::format_mcp_list;
pub(crate) use picker::mcp_picker;

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
    let obj = config
        .as_object_mut()
        .ok_or_else(|| format!("{} must hold an MCP object", path.display()))?;
    obj.entry("mcpServers").or_insert_with(|| json!({}));
    obj.entry("disabledServers").or_insert_with(|| json!([]));
    write_json_value(path, &config)?;
    Ok(path.to_path_buf())
}

fn head_rest(parts: &[String]) -> (&str, &[String]) {
    match parts.split_first() {
        Some((first, rest)) => (first.as_str(), rest),
        None => ("", &[]),
    }
}

/// The `/mcp` verbs. A word that names none of them is refused with the
/// usage.
#[derive(Clone, Copy, Deserialize)]
#[serde(rename_all = "lowercase")]
enum McpVerb {
    List,
    Status,
    Add,
    Remove,
    Enable,
    Disable,
    Tools,
    Resources,
    Prompts,
    Test,
    Notifications,
    Reload,
    Reconnect,
}

/// Validate the selected scope before any mutation, preserving unrelated fields.
fn mcp_scope_config(path: &Path) -> Result<Value, String> {
    mcp::read_config_file(path)
}

const MCP_USAGE: &str = "Usage: /mcp list | status | add [--scope user|project] <name> [stdio <command> [args...] | http <url>] | remove [--scope user|project] <name> | enable [--scope user|project] <name> | disable [--scope user|project] <name> | tools <server> | resources <server> | prompts <server> | notifications <server> | test <server> | reload | reconnect <server>";

pub(crate) fn handle_mcp(args: &str, context: &SlashContext<'_>) -> Result<String, String> {
    let (verb, rest) = split_head(args);
    let verb = if verb.is_empty() { "list" } else { verb };
    let Ok(parsed) = serde_json::from_value::<McpVerb>(Value::from(verb)) else {
        return Err(MCP_USAGE.into());
    };
    match parsed {
        McpVerb::List => format_mcp_list(context.cwd),
        McpVerb::Status => serde_json::to_string_pretty(&mcp::manager_status(context.cwd)?)
            .map_err(|error| error.to_string()),
        McpVerb::Add => {
            let (_force, scope, parts) = parse_marketplace_flags(&split_args(rest));
            let scope = normalize_scope(scope)?;
            let (name, after_name) = head_rest(&parts);
            if !valid_mcp_server_name(name) {
                return Err("Usage: /mcp add [--scope user|project] <name> [stdio <command> [args...] | http <url>]".into());
            }
            let (transport_or_command, after_transport) = head_rest(after_name);
            if transport_or_command.is_empty() {
                return Err("Usage: /mcp add [--scope user|project] <name> [stdio <command> [args...] | http <url>]".into());
            }
            let server = match transport_or_command {
                "http" | "streamable-http" => {
                    let (url, extra) = head_rest(after_transport);
                    if url.is_empty()
                        || !extra.is_empty()
                        || !(url.starts_with("http://") || url.starts_with("https://"))
                    {
                        return Err(format!("Usage: /mcp add {name} http <http(s)-url>"));
                    }
                    json!({"type": "streamable-http", "url": url})
                }
                "sse" => {
                    return Err(
                        "Legacy SSE transport is not supported; use streamable HTTP.".into(),
                    )
                }
                "stdio" => {
                    let (command, after_command) = head_rest(after_transport);
                    if command.is_empty() {
                        return Err(format!("Usage: /mcp add {name} stdio <command> [args...]"));
                    }
                    json!({"type": "stdio", "command": command, "args": after_command.to_vec()})
                }
                command => {
                    json!({"type": "stdio", "command": command, "args": after_transport.to_vec()})
                }
            };
            let path = mcp_scope_file(context.cwd, &scope)?;
            let mut config = mcp_scope_config(&path)?;
            config
                .as_object_mut()
                .expect("checked object")
                .entry("mcpServers")
                .or_insert_with(|| json!({}));
            config["mcpServers"][name] = server;
            let path = mcp_write_file(&path, config)?;
            mcp::manager_status(context.cwd)?;
            Ok(format!(
                "Added MCP server {name} in {scope} scope ({})",
                path.display()
            ))
        }
        McpVerb::Remove => {
            let (_force, scope, parts) = parse_marketplace_flags(&split_args(rest));
            let scope = normalize_scope(scope)?;
            let Some(name) = parts
                .first()
                .map(String::as_str)
                .filter(|name| !name.is_empty())
            else {
                return Err("Usage: /mcp remove [--scope user|project] <name>".into());
            };
            let path = mcp_scope_file(context.cwd, &scope)?;
            let mut config = mcp_scope_config(&path)?;
            let removed = config
                .get_mut("mcpServers")
                .and_then(Value::as_object_mut)
                .and_then(|servers| servers.remove(name))
                .is_some();
            if let Some(disabled) = config
                .get_mut("disabledServers")
                .and_then(Value::as_array_mut)
            {
                disabled.retain(|value| value.as_str() != Some(name));
            }
            let path = mcp_write_file(&path, config)?;
            mcp::manager_status(context.cwd)?;
            if removed {
                Ok(format!(
                    "Removed MCP server {name} from {scope} scope ({})",
                    path.display()
                ))
            } else {
                Err(format!("MCP server not found in {scope} scope: {name}"))
            }
        }
        McpVerb::Enable | McpVerb::Disable => {
            let disable = matches!(parsed, McpVerb::Disable);
            let (_force, scope, parts) = parse_marketplace_flags(&split_args(rest));
            let scope = normalize_scope(scope)?;
            let Some(name) = parts
                .first()
                .map(String::as_str)
                .filter(|name| !name.is_empty())
            else {
                return Err(format!("Usage: /mcp {verb} [--scope user|project] <name>"));
            };
            let path = mcp_scope_file(context.cwd, &scope)?;
            let mut config = mcp_scope_config(&path)?;
            config
                .as_object_mut()
                .expect("checked object")
                .entry("disabledServers")
                .or_insert_with(|| json!([]));
            let disabled = config
                .get_mut("disabledServers")
                .and_then(Value::as_array_mut)
                .ok_or("disabledServers must be an array")?;
            if disable {
                if !disabled.iter().any(|value| value.as_str() == Some(name)) {
                    disabled.push(json!(name));
                }
            } else {
                disabled.retain(|value| value.as_str() != Some(name));
                if let Some(server) = config
                    .get_mut("mcpServers")
                    .and_then(Value::as_object_mut)
                    .and_then(|servers| servers.get_mut(name))
                    .and_then(Value::as_object_mut)
                {
                    if server.get("enabled").and_then(Value::as_bool) == Some(false) {
                        server.insert("enabled".into(), json!(true));
                    }
                }
            }
            let path = mcp_write_file(&path, config)?;
            mcp::manager_status(context.cwd)?;
            Ok(format!(
                "{} MCP server {name} in {scope} scope ({})",
                if disable { "Disabled" } else { "Enabled" },
                path.display()
            ))
        }
        McpVerb::Tools | McpVerb::Resources | McpVerb::Prompts | McpVerb::Test => {
            let (server, _) = split_head(rest);
            if server.is_empty() {
                return Err(format!("Usage: /mcp {} <server>", verb));
            }
            let result = match parsed {
                McpVerb::Resources => mcp::list_resources(context.cwd, server),
                McpVerb::Prompts => mcp::list_prompts(context.cwd, server),
                _ => mcp::list_tools(context.cwd, server),
            }?;
            serde_json::to_string_pretty(&result).map_err(|e| e.to_string())
        }
        McpVerb::Notifications => {
            let (server, _) = split_head(rest);
            if server.is_empty() {
                return Err("Usage: /mcp notifications <server>".into());
            }
            let init = mcp::server_capabilities(context.cwd, server)?;
            let capabilities = init.get("capabilities").ok_or_else(|| {
                format!("MCP server {server} answered initialize without a capabilities object")
            })?;
            Ok(format!(
                "MCP notification capabilities for {} (live persistent connection):\n{}",
                server,
                serde_json::to_string_pretty(capabilities).map_err(|e| e.to_string())?
            ))
        }
        McpVerb::Reload => {
            let status = mcp::refresh_all(context.cwd)?;
            Ok(format!(
                "Reloaded MCP config and refreshed live connections:\n{}",
                serde_json::to_string_pretty(&status).map_err(|error| error.to_string())?
            ))
        }
        McpVerb::Reconnect => {
            let (server, _) = split_head(rest);
            if server.is_empty() {
                return Err("Usage: /mcp reconnect <server>".into());
            }
            let status = mcp::reconnect(context.cwd, server)
                .map_err(|error| format!("Reconnect to {server} failed: {error}"))?;
            Ok(format!(
                "Reconnected MCP server {server}:\n{}",
                serde_json::to_string_pretty(&status).map_err(|error| error.to_string())?
            ))
        }
    }
}
