//! `/mcp list`: every configured server with whether it is enabled, the
//! scope that declares it and the transport it speaks.

use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

use crate::mcp;
use crate::slash::common::{dirs_home, read_json_value};

/// Whether `config` declares a server named `name`.
fn declares(config: &Value, name: &str) -> bool {
    config
        .get("mcpServers")
        .and_then(Value::as_object)
        .is_some_and(|servers| servers.contains_key(name))
}

pub(super) fn format_mcp_list(cwd: &Path) -> Result<String, String> {
    let user = read_json_value(&dirs_home().join(".jeden/mcp.json"))?;
    let project = read_json_value(&cwd.join(".jeden/mcp.json"))?;
    let merged = mcp::load_config(cwd)?;
    let disabled = merged
        .get("disabledServers")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .collect::<BTreeSet<_>>();
    let Some(servers) = merged.get("mcpServers").and_then(Value::as_object) else {
        return Ok("No MCP servers configured.".into());
    };
    if servers.is_empty() {
        return Ok("No MCP servers configured.".into());
    }
    let mut lines = Vec::new();
    for (name, server) in BTreeMap::from_iter(servers.iter()) {
        let source = if declares(&project, name) {
            "project"
        } else if declares(&user, name) {
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
        // A server that names no type is read the way the MCP client reads
        // it: one with a `url` speaks HTTP, any other runs a command.
        let transport = match server.get("type").and_then(Value::as_str) {
            Some(declared) => declared,
            None if server.get("url").is_some() => "http",
            None => "stdio",
        };
        lines.push(format!("{name} [{status}, {source}, {transport}]"));
    }
    Ok(lines.join("\n"))
}
