use super::ToolInfo;
use serde_json::{json, Value};
use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

fn native_mcp_tool_name(server_name: &str, tool_name: &str) -> String {
    fn safe(value: &str) -> String {
        value.chars().map(|ch| if ch.is_ascii_alphanumeric() || ch == '_' { ch } else { '_' }).collect()
    }
    format!("mcp__{}__{}", safe(server_name), safe(tool_name))
}

fn merge_mcp_config(cwd: &Path) -> Value {
    crate::mcp::load_config(cwd)
}

pub(super) fn static_mcp_tools(cwd: &Path, seen: &mut BTreeSet<String>) -> Vec<ToolInfo> {
    let config = merge_mcp_config(cwd);
    let disabled = config.get("disabledServers").and_then(Value::as_array).into_iter().flatten().filter_map(Value::as_str).collect::<BTreeSet<_>>();
    let mut out = Vec::new();
    let Some(servers) = config.get("mcpServers").and_then(Value::as_object) else { return out };
    let ordered: BTreeMap<_, _> = servers.iter().collect();
    for (server_name, server) in ordered {
        if disabled.contains(server_name.as_str()) { continue; }
        let Some(tools) = server.get("tools").and_then(Value::as_array) else { continue; };
        for tool in tools {
            let Some(raw_name) = tool.get("name").and_then(Value::as_str) else { continue; };
            let native_name = native_mcp_tool_name(server_name, raw_name);
            if seen.contains(&native_name) { continue; }
            let description = tool.get("description").and_then(Value::as_str).map(ToString::to_string).unwrap_or_else(|| format!("MCP tool {} from {}", raw_name, server_name));
            seen.insert(native_name.clone());
            out.push(ToolInfo { name: native_name, description, input: tool.get("input").cloned().unwrap_or_else(|| json!({})) });
        }
    }
    out
}

pub fn native_mcp_tool_target(cwd: &Path, native_name: &str) -> Option<(String, String)> {
    let config = merge_mcp_config(cwd);
    let disabled = config.get("disabledServers").and_then(Value::as_array).into_iter().flatten().filter_map(Value::as_str).collect::<BTreeSet<_>>();
    let servers = config.get("mcpServers").and_then(Value::as_object)?;
    let ordered: BTreeMap<_, _> = servers.iter().collect();
    for (server_name, server) in ordered {
        if disabled.contains(server_name.as_str()) { continue; }
        let Some(tools) = server.get("tools").and_then(Value::as_array) else { continue };
        for tool in tools {
            let Some(raw_name) = tool.get("name").and_then(Value::as_str) else { continue };
            if native_mcp_tool_name(server_name, raw_name) == native_name {
                return Some((server_name.clone(), raw_name.to_string()));
            }
        }
    }
    None
}
