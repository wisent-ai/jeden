use serde_json::{json, Value};
use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Arc, LazyLock, Mutex, MutexGuard};
use std::thread;
use std::time::{Duration, Instant};

mod client;

use crate::capability::{
    CapabilityDescriptor, CapabilityHealth, CapabilityKind, CapabilityPolicy, FunctionTarget,
};
use client::McpClient;

const MCP_PROTOCOL_VERSION: &str = "2024-11-05";
const MAX_STDERR_BYTES: usize = 100_000;
const CIRCUIT_FAILURE_LIMIT: u32 = 5;
const CIRCUIT_OPEN: Duration = Duration::from_secs(30);

fn dirs_home() -> PathBuf {
    env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("."))
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

fn configured_servers(cwd: &Path) -> Result<BTreeMap<String, Value>, String> {
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

#[derive(Clone, Copy)]
enum ConnectionState {
    Disconnected,
    Connecting,
    Ready,
    Backoff,
    CircuitOpen,
}

impl ConnectionState {
    fn as_str(self) -> &'static str {
        match self {
            Self::Disconnected => "disconnected",
            Self::Connecting => "connecting",
            Self::Ready => "ready",
            Self::Backoff => "backoff",
            Self::CircuitOpen => "circuit-open",
        }
    }
}

struct ServerConnection {
    config: Value,
    client: Option<McpClient>,
    state: ConnectionState,
    failures: u32,
    retry_after: Option<Instant>,
    last_error: Option<String>,
    initialize: Value,
    tools: Value,
    resources: Value,
    prompts: Value,
}

impl ServerConnection {
    fn new(config: Value) -> Self {
        Self {
            config,
            client: None,
            state: ConnectionState::Disconnected,
            failures: 0,
            retry_after: None,
            last_error: None,
            initialize: Value::Null,
            tools: json!({"tools": []}),
            resources: json!({"resources": []}),
            prompts: json!({"prompts": []}),
        }
    }

    fn disconnect(&mut self) {
        if let Some(mut client) = self.client.take() {
            client.close();
        }
        self.state = ConnectionState::Disconnected;
    }

    fn record_failure(&mut self, error: String) {
        self.disconnect();
        self.failures = self.failures.saturating_add(1);
        self.last_error = Some(error);
        let delay = if self.failures >= CIRCUIT_FAILURE_LIMIT {
            self.state = ConnectionState::CircuitOpen;
            CIRCUIT_OPEN
        } else {
            self.state = ConnectionState::Backoff;
            Duration::from_millis(100_u64.saturating_mul(1 << self.failures.min(4)))
        };
        self.retry_after = Some(Instant::now() + delay);
    }

    fn connect(&mut self, cwd: &Path, timeout_ms: u64, force: bool) -> Result<(), String> {
        if self.client.as_mut().map(McpClient::is_alive) == Some(true) && !force {
            return Ok(());
        }
        self.disconnect();
        if !force {
            if let Some(retry_after) = self.retry_after {
                if retry_after > Instant::now() && self.failures >= CIRCUIT_FAILURE_LIMIT {
                    self.state = ConnectionState::CircuitOpen;
                    return Err(format!(
                        "MCP circuit is open for {}ms",
                        retry_after.duration_since(Instant::now()).as_millis()
                    ));
                }
                if retry_after > Instant::now() {
                    thread::sleep(
                        retry_after
                            .duration_since(Instant::now())
                            .min(Duration::from_secs(2)),
                    );
                }
            }
        }
        self.state = ConnectionState::Connecting;
        let result: Result<(), String> = (|| {
            let mut client = McpClient::start(&self.config, cwd)?;
            let initialize = client.initialize(timeout_ms)?;
            let capabilities = initialize
                .get("capabilities")
                .and_then(Value::as_object)
                .ok_or("MCP initialize capabilities must be an object")?;
            let tools = if capabilities.contains_key("tools") {
                let value = client.request("tools/list", json!({}), timeout_ms)?;
                validate_tools(&value)?;
                value
            } else {
                json!({"tools": []})
            };
            let resources = if capabilities.contains_key("resources") {
                let value = client.request("resources/list", json!({}), timeout_ms)?;
                validate_resources(&value)?;
                value
            } else {
                json!({"resources": []})
            };
            let prompts = if capabilities.contains_key("prompts") {
                let value = client.request("prompts/list", json!({}), timeout_ms)?;
                validate_prompts(&value)?;
                value
            } else {
                json!({"prompts": []})
            };
            self.initialize = initialize;
            self.tools = tools;
            self.resources = resources;
            self.prompts = prompts;
            self.client = Some(client);
            Ok(())
        })();
        match result {
            Ok(()) => {
                self.state = ConnectionState::Ready;
                self.failures = 0;
                self.retry_after = None;
                self.last_error = None;
                Ok(())
            }
            Err(error) => {
                self.record_failure(error.clone());
                Err(error)
            }
        }
    }

    fn request(
        &mut self,
        cwd: &Path,
        method: &str,
        params: Value,
        timeout_ms: u64,
    ) -> Result<Value, String> {
        self.connect(cwd, timeout_ms, false)?;
        let first = self
            .client
            .as_mut()
            .ok_or("MCP connection unavailable")?
            .request(method, params.clone(), timeout_ms);
        let result = match first {
            Ok(value) => Ok(value),
            Err(error) => {
                let dead = self
                    .client
                    .as_mut()
                    .map(|client| !client.is_alive())
                    .unwrap_or(true);
                let transport_error = dead
                    || error.contains("transport")
                    || error.contains("stdio")
                    || error.contains("exceeded");
                if !transport_error {
                    return Err(error);
                }
                self.record_failure(error);
                self.connect(cwd, timeout_ms, false)?;
                let retry = self
                    .client
                    .as_mut()
                    .ok_or("MCP connection unavailable")?
                    .request(method, params, timeout_ms);
                if let Err(error) = &retry {
                    let dead = self
                        .client
                        .as_mut()
                        .map(|client| !client.is_alive())
                        .unwrap_or(true);
                    if dead
                        || error.contains("transport")
                        || error.contains("stdio")
                        || error.contains("exceeded")
                    {
                        self.record_failure(error.clone());
                    }
                }
                retry
            }
        }?;
        let notifications = self
            .client
            .as_mut()
            .map(McpClient::take_notifications)
            .unwrap_or_default();
        self.process_notifications(cwd, notifications, timeout_ms)?;
        Ok(result)
    }

    fn process_notifications(
        &mut self,
        _cwd: &Path,
        notifications: Vec<Value>,
        timeout_ms: u64,
    ) -> Result<(), String> {
        let mut tools_changed = false;
        let mut resources_changed = false;
        let mut prompts_changed = false;
        for notification in notifications {
            match notification.get("method").and_then(Value::as_str) {
                Some("notifications/tools/list_changed") => tools_changed = true,
                Some("notifications/resources/list_changed") => resources_changed = true,
                Some("notifications/prompts/list_changed") => prompts_changed = true,
                Some("notifications/message")
                | Some("notifications/progress")
                | Some("notifications/resources/updated")
                | Some("notifications/cancelled") => {}
                Some(method) => {
                    return Err(format!("unsupported MCP notification method: {method}"))
                }
                None => return Err("MCP notification is missing method".into()),
            }
        }
        let client = self.client.as_mut().ok_or("MCP connection unavailable")?;
        if tools_changed {
            let value = client.request("tools/list", json!({}), timeout_ms)?;
            validate_tools(&value)?;
            self.tools = value;
        }
        if resources_changed {
            let value = client.request("resources/list", json!({}), timeout_ms)?;
            validate_resources(&value)?;
            self.resources = value;
        }
        if prompts_changed {
            let value = client.request("prompts/list", json!({}), timeout_ms)?;
            validate_prompts(&value)?;
            self.prompts = value;
        }
        if tools_changed || resources_changed || prompts_changed {
            crate::capability::invalidate();
        }
        Ok(())
    }
}

impl Drop for ServerConnection {
    fn drop(&mut self) {
        self.disconnect();
    }
}

#[derive(Default)]
struct McpManager {
    servers: BTreeMap<String, ServerConnection>,
}

impl McpManager {
    fn sync_config(&mut self, cwd: &Path) -> Result<(), String> {
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

fn session_key(cwd: &Path) -> PathBuf {
    cwd.canonicalize().unwrap_or_else(|_| cwd.to_path_buf())
}

fn managers() -> Result<MutexGuard<'static, BTreeMap<PathBuf, McpManager>>, String> {
    MANAGERS
        .lock()
        .map_err(|_| "MCP manager lock is poisoned".to_string())
}

fn with_connection<T>(
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

fn validate_tools(value: &Value) -> Result<(), String> {
    let tools = value
        .get("tools")
        .and_then(Value::as_array)
        .ok_or("MCP tools/list result must contain a tools array")?;
    for tool in tools {
        if tool
            .get("name")
            .and_then(Value::as_str)
            .filter(|name| !name.is_empty())
            .is_none()
        {
            return Err("MCP tool.name must be a non-empty string".into());
        }
        if !tool
            .get("inputSchema")
            .map(Value::is_object)
            .unwrap_or(false)
        {
            return Err("MCP tool.inputSchema must be an object".into());
        }
    }
    Ok(())
}

fn validate_resources(value: &Value) -> Result<(), String> {
    let resources = value
        .get("resources")
        .and_then(Value::as_array)
        .ok_or("MCP resources/list result must contain a resources array")?;
    for resource in resources {
        if resource
            .get("uri")
            .and_then(Value::as_str)
            .filter(|uri| !uri.is_empty())
            .is_none()
            || resource
                .get("name")
                .and_then(Value::as_str)
                .filter(|name| !name.is_empty())
                .is_none()
        {
            return Err("MCP resource uri and name must be non-empty strings".into());
        }
    }
    Ok(())
}

fn validate_prompts(value: &Value) -> Result<(), String> {
    let prompts = value
        .get("prompts")
        .and_then(Value::as_array)
        .ok_or("MCP prompts/list result must contain a prompts array")?;
    for prompt in prompts {
        if prompt
            .get("name")
            .and_then(Value::as_str)
            .filter(|name| !name.is_empty())
            .is_none()
        {
            return Err("MCP prompt.name must be a non-empty string".into());
        }
    }
    Ok(())
}

pub fn list_tools(cwd: &Path, server_name: &str, timeout_ms: u64) -> Result<Value, String> {
    with_connection(cwd, server_name, |connection| {
        let value = connection.request(cwd, "tools/list", json!({}), timeout_ms)?;
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
    timeout_ms: u64,
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
            timeout_ms,
        )?;
        if !value.is_object() {
            return Err("MCP tools/call result must be an object".into());
        }
        Ok(value)
    })
}

pub fn list_resources(cwd: &Path, server_name: &str, timeout_ms: u64) -> Result<Value, String> {
    with_connection(cwd, server_name, |connection| {
        let value = connection.request(cwd, "resources/list", json!({}), timeout_ms)?;
        validate_resources(&value)?;
        connection.resources = value.clone();
        Ok(value)
    })
}

pub fn read_resource(
    cwd: &Path,
    server_name: &str,
    uri: &str,
    timeout_ms: u64,
) -> Result<Value, String> {
    if uri.is_empty() {
        return Err("uri is required".into());
    }
    with_connection(cwd, server_name, |connection| {
        let value = connection.request(cwd, "resources/read", json!({"uri": uri}), timeout_ms)?;
        if !value.is_object() {
            return Err("MCP resources/read result must be an object".into());
        }
        Ok(value)
    })
}

pub fn list_prompts(cwd: &Path, server_name: &str, timeout_ms: u64) -> Result<Value, String> {
    with_connection(cwd, server_name, |connection| {
        let value = connection.request(cwd, "prompts/list", json!({}), timeout_ms)?;
        validate_prompts(&value)?;
        connection.prompts = value.clone();
        Ok(value)
    })
}

pub fn server_capabilities(
    cwd: &Path,
    server_name: &str,
    timeout_ms: u64,
) -> Result<Value, String> {
    with_connection(cwd, server_name, |connection| {
        connection.connect(cwd, timeout_ms, false)?;
        Ok(connection.initialize.clone())
    })
}

pub fn get_prompt(
    cwd: &Path,
    server_name: &str,
    name: &str,
    args: Value,
    timeout_ms: u64,
) -> Result<Value, String> {
    if name.is_empty() {
        return Err("name is required".into());
    }
    if !args.is_object() {
        return Err("MCP prompt arguments must be an object".into());
    }
    with_connection(cwd, server_name, |connection| {
        let value = connection.request(
            cwd,
            "prompts/get",
            json!({"name": name, "arguments": args}),
            timeout_ms,
        )?;
        if !value.is_object() {
            return Err("MCP prompts/get result must be an object".into());
        }
        Ok(value)
    })
}

/// One handshake per server: its name, the connection, and how the handshake
/// went.
type Handshakes = Vec<(String, ServerConnection, Result<(), String>)>;

fn connect_parallel(
    pending: Vec<(String, ServerConnection)>,
    cwd: &Path,
    timeout_ms: u64,
    force: bool,
) -> Result<Handshakes, String> {
    if pending.is_empty() {
        return Ok(Vec::new());
    }
    let worker_count = pending
        .len()
        .min(thread::available_parallelism().map_or(1, usize::from))
        .min(8);
    let queue = Arc::new(Mutex::new(VecDeque::from(pending)));
    thread::scope(|scope| {
        let handles = (0..worker_count)
            .map(|_| {
                let queue = Arc::clone(&queue);
                scope.spawn(move || {
                    let mut completed = Vec::new();
                    loop {
                        let item = queue
                            .lock()
                            .map_err(|_| "MCP startup queue lock is poisoned".to_string())?
                            .pop_front();
                        let Some((name, mut connection)) = item else {
                            break;
                        };
                        let result = connection.connect(cwd, timeout_ms, force);
                        completed.push((name, connection, result));
                    }
                    Ok::<_, String>(completed)
                })
            })
            .collect::<Vec<_>>();
        let mut completed = Vec::new();
        for handle in handles {
            completed.extend(
                handle
                    .join()
                    .map_err(|_| "MCP startup worker panicked".to_string())??,
            );
        }
        Ok(completed)
    })
}

pub fn live_tools(cwd: &Path, timeout_ms: u64) -> Result<Vec<(String, Value)>, String> {
    let key = session_key(cwd);
    let pending = {
        let mut managers = managers()?;
        let manager = managers.entry(key.clone()).or_default();
        manager.sync_config(cwd)?;
        for connection in manager.servers.values_mut() {
            let notifications = match connection.client.as_mut() {
                Some(client) => {
                    if client.is_alive() {
                        client.poll_notifications()
                    } else {
                        continue;
                    }
                }
                None => continue,
            };
            match notifications {
                Ok(notifications) => {
                    if let Err(error) =
                        connection.process_notifications(cwd, notifications, timeout_ms)
                    {
                        connection.record_failure(error);
                    }
                }
                Err(error) => connection.record_failure(error),
            }
        }
        let names = manager
            .servers
            .iter_mut()
            .filter_map(|(name, connection)| {
                let alive = connection
                    .client
                    .as_mut()
                    .map(McpClient::is_alive)
                    .unwrap_or(false);
                (!alive).then(|| name.clone())
            })
            .collect::<Vec<_>>();
        names
            .into_iter()
            .filter_map(|name| {
                manager
                    .servers
                    .remove(&name)
                    .map(|connection| (name, connection))
            })
            .collect::<Vec<_>>()
    };
    let results = connect_parallel(pending, cwd, timeout_ms, false)?;
    let mut startup_errors = Vec::new();
    let mut managers = managers()?;
    let manager = managers.entry(key).or_default();
    for (name, connection, result) in results {
        if let Err(error) = result {
            startup_errors.push(format!("{name}: {error}"));
        }
        manager.servers.insert(name, connection);
    }
    let tools = manager
        .servers
        .iter()
        .filter(|(_, connection)| matches!(connection.state, ConnectionState::Ready))
        .flat_map(|(server, connection)| {
            connection
                .tools
                .get("tools")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
                .cloned()
                .map(move |tool| (server.clone(), tool))
        })
        .collect::<Vec<_>>();
    if tools.is_empty() && !startup_errors.is_empty() {
        return Err(format!(
            "no MCP server reached ready state: {}",
            startup_errors.join("; ")
        ));
    }
    Ok(tools)
}

pub fn refresh_all(cwd: &Path, timeout_ms: u64) -> Result<Value, String> {
    let configs = configured_servers(cwd)?;
    let pending = configs
        .into_iter()
        .map(|(name, config)| (name, ServerConnection::new(config)))
        .collect();
    let results = connect_parallel(pending, cwd, timeout_ms, true)?;
    let key = session_key(cwd);
    let mut managers = managers()?;
    let manager = managers.entry(key).or_default();
    let mut status = serde_json::Map::new();
    let configured_names = results
        .iter()
        .map(|(name, _, _)| name.clone())
        .collect::<BTreeSet<_>>();
    manager
        .servers
        .retain(|name, _| configured_names.contains(name));
    for (name, connection, result) in results {
        status.insert(name.clone(), match result { Ok(()) => json!({"state": "ready", "tools": connection.tools.get("tools").and_then(Value::as_array).map_or(0, Vec::len)}), Err(error) => json!({"state": connection.state.as_str(), "error": error}) });
        manager.servers.insert(name, connection);
    }
    crate::capability::invalidate();
    Ok(Value::Object(status))
}

pub fn reconnect(cwd: &Path, server_name: &str, timeout_ms: u64) -> Result<Value, String> {
    let config = configured_server(cwd, server_name)?;
    with_connection(cwd, server_name, |connection| {
        connection.disconnect();
        connection.config = config;
        connection.failures = 0;
        connection.retry_after = None;
        connection.connect(cwd, timeout_ms, true)?;
        crate::capability::invalidate();
        Ok(
            json!({"server": server_name, "state": connection.state.as_str(), "tools": connection.tools.get("tools").and_then(Value::as_array).map_or(0, Vec::len)}),
        )
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

pub(crate) fn capability_descriptors(cwd: &Path) -> Vec<CapabilityDescriptor> {
    let sandbox = crate::tool_runtime::runtime_ops::SecureRuntime::detect()
        .health()
        .clone();
    if !sandbox.enforced() {
        return vec![CapabilityDescriptor::new(
            "service/mcp",
            CapabilityKind::Service,
            "jeden-core",
            "MCP manager",
            "Persistent bounded MCP connection manager",
            FunctionTarget::Service { name: "mcp".into() },
        )
        .operation("status")
        .policy(CapabilityPolicy::Sandboxed)
        .health(CapabilityHealth::unavailable(format!(
            "sandbox {} is not enforced: {}",
            sandbox.backend, sandbox.detail
        )))];
    }
    let discovery_error = live_tools(cwd, 30_000).err();
    let key = session_key(cwd);
    let Ok(mut managers) = managers() else {
        return vec![CapabilityDescriptor::new(
            "service/mcp",
            CapabilityKind::Service,
            "jeden-core",
            "MCP manager",
            "Persistent MCP connection manager",
            FunctionTarget::Service { name: "mcp".into() },
        )
        .operation("refresh")
        .health(CapabilityHealth::unavailable(
            "MCP manager lock is poisoned",
        ))];
    };
    let manager = managers.entry(key).or_default();
    if let Err(error) = manager.sync_config(cwd) {
        return vec![CapabilityDescriptor::new(
            "service/mcp",
            CapabilityKind::Service,
            "jeden-core",
            "MCP manager",
            "Persistent MCP connection manager",
            FunctionTarget::Service { name: "mcp".into() },
        )
        .operation("refresh")
        .health(CapabilityHealth::unavailable(error))];
    }
    let mut out = Vec::new();
    out.push(
        CapabilityDescriptor::new(
            "service/mcp",
            CapabilityKind::Service,
            "jeden-core",
            "MCP manager",
            "Persistent bounded MCP connection manager",
            FunctionTarget::Service { name: "mcp".into() },
        )
        .operation("refresh")
        .operation("status")
        .health(match discovery_error {
            Some(error) => CapabilityHealth {
                state: crate::capability::HealthState::Degraded,
                detail: Some(error),
            },
            None => CapabilityHealth::healthy(),
        }),
    );
    for (server, connection) in &manager.servers {
        let ready = matches!(connection.state, ConnectionState::Ready);
        let health = if ready {
            CapabilityHealth::healthy()
        } else {
            CapabilityHealth::unavailable(
                connection
                    .last_error
                    .clone()
                    .unwrap_or_else(|| format!("MCP server is {}", connection.state.as_str())),
            )
        };
        out.push(
            CapabilityDescriptor::new(
                format!("mcp/{server}"),
                CapabilityKind::Mcp,
                format!("mcp:{server}"),
                server.clone(),
                format!("MCP server {server}"),
                FunctionTarget::McpServer {
                    name: server.clone(),
                },
            )
            .operation("tools/list")
            .operation("resources/list")
            .operation("prompts/list")
            .health(health.clone()),
        );
        for tool in connection
            .tools
            .get("tools")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
        {
            let Some(remote_name) = tool.get("name").and_then(Value::as_str) else {
                continue;
            };
            let native_name = crate::tools::native_mcp_tool_name(server, remote_name);
            let description = tool
                .get("description")
                .and_then(Value::as_str)
                .map(str::to_string)
                .unwrap_or_else(|| format!("MCP tool {remote_name} from {server}"));
            out.push(CapabilityDescriptor::new(
                format!("tool/{native_name}"), CapabilityKind::Tool, format!("mcp:{server}"), native_name.clone(), description,
                FunctionTarget::McpTool { native_name: native_name.clone(), server: server.clone(), remote_name: remote_name.into() },
            ).operation("execute").dependency(format!("mcp/{server}")).policy(CapabilityPolicy::Sandboxed)
             .health(health.clone()).executable(native_name).metadata(json!({"input": tool.get("inputSchema").cloned().unwrap_or_else(|| json!({"type":"object"}))})));
        }
    }
    out
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
