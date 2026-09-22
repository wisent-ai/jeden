//! Which external tool servers this workspace has, merged from the project
//! and the account and honouring what is turned off.
//!
//! Split out of `mcp/mod.rs`, which had grown past the module line cap.

use serde_json::{json, Value};
use std::collections::BTreeMap;
use std::collections::BTreeSet;
use std::env;
use std::fs;
use std::path::{Path, PathBuf};

pub(super) fn dirs_home() -> PathBuf {
    env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("."))
}

pub(super) fn read_json_value(path: &Path) -> Value {
    fs::read_to_string(path)
        .ok()
        .and_then(|text| serde_json::from_str::<Value>(&text).ok())
        .unwrap_or(Value::Null)
}

pub fn load_config(cwd: &Path) -> Value {
    let user = read_json_value(&dirs_home().join(".jeden/mcp.json"));
    let project = read_json_value(&cwd.join(".jeden/mcp.json"));
    let mut servers = serde_json::Map::new();
    let mut disabled = Vec::new();
    for source in [&user, &project] {
        if let Some(map) = source.get("mcpServers").and_then(Value::as_object) {
            for (name, server) in map {
                servers.insert(name.clone(), server.clone());
            }
        }
    }
    for (name, server) in &servers {
        if server.get("enabled").and_then(Value::as_bool) == Some(false) {
            disabled.push(name.clone());
        }
    }
    for source in [&user, &project] {
        if let Some(values) = source.get("disabledServers").and_then(Value::as_array) {
            disabled.extend(
                values
                    .iter()
                    .filter_map(Value::as_str)
                    .map(ToOwned::to_owned),
            );
        }
    }
    json!({"mcpServers": servers, "disabledServers": disabled})
}

pub(crate) fn configured_servers(cwd: &Path) -> Result<BTreeMap<String, Value>, String> {
    let config = load_config(cwd);
    let disabled = config
        .get("disabledServers")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .collect::<BTreeSet<_>>();
    let servers = config
        .get("mcpServers")
        .and_then(Value::as_object)
        .ok_or("mcpServers must be an object")?;
    Ok(servers
        .iter()
        .filter(|(name, _)| !disabled.contains(name.as_str()))
        .map(|(name, server)| (name.clone(), server.clone()))
        .collect())
}

pub(crate) fn configured_server(cwd: &Path, server_name: &str) -> Result<Value, String> {
    let config = load_config(cwd);
    let disabled = config
        .get("disabledServers")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .collect::<BTreeSet<_>>();
    if disabled.contains(server_name) {
        return Err(format!("disabled MCP server: {server_name}"));
    }
    config
        .get("mcpServers")
        .and_then(Value::as_object)
        .and_then(|servers| servers.get(server_name))
        .cloned()
        .ok_or_else(|| format!("unknown MCP server: {server_name}"))
}

pub(crate) fn string_field<'a>(value: &'a Value, key: &str) -> Option<&'a str> {
    value.get(key).and_then(Value::as_str)
}

pub(crate) fn resolve_server_cwd(cwd: &Path, server: &Value) -> PathBuf {
    match string_field(server, "cwd") {
        Some(raw) if Path::new(raw).is_absolute() => PathBuf::from(raw),
        Some(raw) => cwd.join(raw),
        None => cwd.to_path_buf(),
    }
}
