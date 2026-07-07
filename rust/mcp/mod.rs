use serde_json::{json, Value};
use std::collections::BTreeSet;
use std::env;
use std::fs;
use std::path::{Path, PathBuf};

mod client;

use client::McpClient;

const MCP_PROTOCOL_VERSION: &str = "2024-11-05";
const MAX_STDERR_BYTES: usize = 100_000;

fn dirs_home() -> PathBuf {
    env::var_os("HOME").map(PathBuf::from).unwrap_or_else(|| PathBuf::from("."))
}

fn read_json_value(path: &Path) -> Value {
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
            disabled.extend(values.iter().filter_map(Value::as_str).map(ToString::to_string));
        }
    }
    json!({"mcpServers": servers, "disabledServers": disabled})
}

fn configured_server(cwd: &Path, server_name: &str) -> Result<Value, String> {
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

fn string_field<'a>(value: &'a Value, key: &str) -> Option<&'a str> {
    value.get(key).and_then(Value::as_str)
}

fn resolve_server_cwd(cwd: &Path, server: &Value) -> PathBuf {
    match string_field(server, "cwd") {
        Some(raw) if Path::new(raw).is_absolute() => PathBuf::from(raw),
        Some(raw) => cwd.join(raw),
        None => cwd.to_path_buf(),
    }
}

fn with_client(cwd: &Path, server_name: &str, timeout_ms: u64, callback: impl FnOnce(&mut McpClient) -> Result<Value, String>) -> Result<Value, String> {
    let server = configured_server(cwd, server_name)?;
    let mut client = McpClient::start(&server, cwd)?;
    client.initialize(timeout_ms)?;
    callback(&mut client)
}

pub fn list_tools(cwd: &Path, server_name: &str, timeout_ms: u64) -> Result<Value, String> {
    with_client(cwd, server_name, timeout_ms, |client| client.request("tools/list", json!({}), timeout_ms))
}

pub fn call_tool(cwd: &Path, server_name: &str, tool_name: &str, args: Value, timeout_ms: u64) -> Result<Value, String> {
    if tool_name.is_empty() {
        return Err("toolName is required".into());
    }
    with_client(cwd, server_name, timeout_ms, |client| client.request("tools/call", json!({"name": tool_name, "arguments": args}), timeout_ms))
}

pub fn list_resources(cwd: &Path, server_name: &str, timeout_ms: u64) -> Result<Value, String> {
    with_client(cwd, server_name, timeout_ms, |client| client.request("resources/list", json!({}), timeout_ms))
}

pub fn read_resource(cwd: &Path, server_name: &str, uri: &str, timeout_ms: u64) -> Result<Value, String> {
    if uri.is_empty() {
        return Err("uri is required".into());
    }
    with_client(cwd, server_name, timeout_ms, |client| client.request("resources/read", json!({"uri": uri}), timeout_ms))
}

pub fn list_prompts(cwd: &Path, server_name: &str, timeout_ms: u64) -> Result<Value, String> {
    with_client(cwd, server_name, timeout_ms, |client| client.request("prompts/list", json!({}), timeout_ms))
}

/// The server's declared capabilities from the initialize handshake — the
/// faithful source for `/mcp notifications` (a stateless one-shot client holds
/// no live subscriptions, so it reports what the server advertises: the
/// `listChanged`/`subscribe`/`logging` notification-related capability flags).
pub fn server_capabilities(cwd: &Path, server_name: &str, timeout_ms: u64) -> Result<Value, String> {
    let server = configured_server(cwd, server_name)?;
    let mut client = McpClient::start(&server, cwd)?;
    client.initialize(timeout_ms)
}

pub fn get_prompt(cwd: &Path, server_name: &str, name: &str, args: Value, timeout_ms: u64) -> Result<Value, String> {
    if name.is_empty() {
        return Err("name is required".into());
    }
    with_client(cwd, server_name, timeout_ms, |client| client.request("prompts/get", json!({"name": name, "arguments": args}), timeout_ms))
}
