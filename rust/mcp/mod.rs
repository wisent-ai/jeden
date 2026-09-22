//! External tool servers: which ones this workspace has, connecting to them,
//! and asking them for tools, resources and prompts.

use serde_json::{json, Value};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::{LazyLock, Mutex, MutexGuard};

mod client;
mod connection;
mod servers;
mod sweep;
mod validate;

use connection::ServerConnection;
pub(crate) use servers::capability_descriptors;
pub use servers::load_config;
use servers::configured_servers;
pub use sweep::{live_tools, reconnect, refresh_all};
use validate::{validate_prompts, validate_resources, validate_tools};

const MCP_PROTOCOL_VERSION: &str = "2024-11-05";
const MAX_STDERR_BYTES: usize = 100_000;

#[derive(Default)]
pub(super) struct McpManager {
    pub(super) servers: BTreeMap<String, ServerConnection>,
}

impl McpManager {
    pub(super) fn sync_config(&mut self, cwd: &Path) -> Result<(), String> {
        let configured = configured_servers(cwd)?;
        self.servers.retain(|name, _| configured.contains_key(name));
        for (name, config) in configured {
            match self.servers.get_mut(&name) {
                Some(connection) if connection.config != config => {
                    connection.disconnect();
                    *connection = ServerConnection::new(config);
                }
                Some(_) => {}
                None => {
                    self.servers.insert(name, ServerConnection::new(config));
                }
            }
        }
        Ok(())
    }
}

static MANAGERS: LazyLock<Mutex<BTreeMap<PathBuf, McpManager>>> =
    LazyLock::new(|| Mutex::new(BTreeMap::new()));

pub(super) fn session_key(cwd: &Path) -> PathBuf {
    cwd.canonicalize().unwrap_or_else(|_| cwd.to_path_buf())
}

pub(super) fn managers() -> Result<MutexGuard<'static, BTreeMap<PathBuf, McpManager>>, String> {
    MANAGERS
        .lock()
        .map_err(|_| "MCP manager lock is poisoned".to_string())
}

pub(super) fn with_connection<T>(
    cwd: &Path,
    server_name: &str,
    operation: impl FnOnce(&mut ServerConnection) -> Result<T, String>,
) -> Result<T, String> {
    let key = session_key(cwd);
    let mut managers = managers()?;
    let manager = managers.entry(key).or_default();
    manager.sync_config(cwd)?;
    let connection = manager
        .servers
        .get_mut(server_name)
        .ok_or_else(|| format!("unknown or disabled MCP server: {server_name}"))?;
    operation(connection)
}

pub fn list_tools(cwd: &Path, server_name: &str) -> Result<Value, String> {
    with_connection(cwd, server_name, |connection| {
        let value = connection.request(cwd, "tools/list", json!({}))?;
        validate_tools(&value)?;
        connection.tools = value.clone();
        Ok(value)
    })
}

pub fn call_tool(
    cwd: &Path,
    server_name: &str,
    tool_name: &str,
    args: Value,
) -> Result<Value, String> {
    if tool_name.is_empty() {
        return Err("toolName is required".into());
    }
    if !args.is_object() {
        return Err("MCP tool arguments must be an object".into());
    }
    with_connection(cwd, server_name, |connection| {
        let value = connection.request(
            cwd,
            "tools/call",
            json!({"name": tool_name, "arguments": args}),
        )?;
        if !value.is_object() {
            return Err("MCP tools/call result must be an object".into());
        }
        Ok(value)
    })
}

pub fn list_resources(cwd: &Path, server_name: &str) -> Result<Value, String> {
    with_connection(cwd, server_name, |connection| {
        let value = connection.request(cwd, "resources/list", json!({}))?;
        validate_resources(&value)?;
        connection.resources = value.clone();
        Ok(value)
    })
}

pub fn read_resource(cwd: &Path, server_name: &str, uri: &str) -> Result<Value, String> {
    if uri.is_empty() {
        return Err("uri is required".into());
    }
    with_connection(cwd, server_name, |connection| {
        let value = connection.request(cwd, "resources/read", json!({"uri": uri}))?;
        if !value.is_object() {
            return Err("MCP resources/read result must be an object".into());
        }
        Ok(value)
    })
}

pub fn list_prompts(cwd: &Path, server_name: &str) -> Result<Value, String> {
    with_connection(cwd, server_name, |connection| {
        let value = connection.request(cwd, "prompts/list", json!({}))?;
        validate_prompts(&value)?;
        connection.prompts = value.clone();
        Ok(value)
    })
}

pub fn server_capabilities(cwd: &Path, server_name: &str) -> Result<Value, String> {
    with_connection(cwd, server_name, |connection| {
        connection.connect(cwd, false)?;
        Ok(connection.initialize.clone())
    })
}

pub fn get_prompt(cwd: &Path, server_name: &str, name: &str, args: Value) -> Result<Value, String> {
    if name.is_empty() {
        return Err("name is required".into());
    }
    if !args.is_object() {
        return Err("MCP prompt arguments must be an object".into());
    }
    with_connection(cwd, server_name, |connection| {
        let value =
            connection.request(cwd, "prompts/get", json!({"name": name, "arguments": args}))?;
        if !value.is_object() {
            return Err("MCP prompts/get result must be an object".into());
        }
        Ok(value)
    })
}


pub fn manager_status(cwd: &Path) -> Result<Value, String> {
    let key = session_key(cwd);
    let mut managers = managers()?;
    let manager = managers.entry(key).or_default();
    manager.sync_config(cwd)?;
    Ok(Value::Object(manager.servers.iter().map(|(name, connection)| (name.clone(), json!({
        "state": connection.state.as_str(),
        "failures": connection.failures,
        "lastError": connection.last_error,
        "tools": connection.tools.get("tools").and_then(Value::as_array).map_or(0, Vec::len),
        "resources": connection.resources.get("resources").and_then(Value::as_array).map_or(0, Vec::len),
        "prompts": connection.prompts.get("prompts").and_then(Value::as_array).map_or(0, Vec::len),
    }))).collect()))
}

pub fn shutdown(cwd: &Path) -> Result<(), String> {
    let key = session_key(cwd);
    if let Some(mut manager) = managers()?.remove(&key) {
        for connection in manager.servers.values_mut() {
            connection.disconnect();
        }
    }
    crate::capability::invalidate();
    Ok(())
}
