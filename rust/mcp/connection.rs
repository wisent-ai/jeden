//! One server's connection, and the state it is in.
//!
//! Split out of `mcp/mod.rs`, which had grown past the module line cap.

use super::client::McpClient;
use super::servers::{resolve_server_cwd, string_field};
use serde_json::{json, Value};
use std::collections::VecDeque;
use std::path::Path;
use std::time::{Duration, Instant};
use crate::mcp::validate::validate_prompts;
use crate::mcp::validate::validate_resources;
use crate::mcp::validate::validate_tools;
use std::thread;

const CIRCUIT_FAILURE_LIMIT: u32 = 5;
const CIRCUIT_OPEN: Duration = Duration::from_secs(30);

#[derive(Clone, Copy)]
pub(crate) enum ConnectionState {
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

pub(super) struct ServerConnection {
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

    fn connect(&mut self, cwd: &Path, force: bool) -> Result<(), String> {
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
            let initialize = client.initialize()?;
            let capabilities = initialize
                .get("capabilities")
                .and_then(Value::as_object)
                .ok_or("MCP initialize capabilities must be an object")?;
            let tools = if capabilities.contains_key("tools") {
                let value = client.request("tools/list", json!({}))?;
                validate_tools(&value)?;
                value
            } else {
                json!({"tools": []})
            };
            let resources = if capabilities.contains_key("resources") {
                let value = client.request("resources/list", json!({}))?;
                validate_resources(&value)?;
                value
            } else {
                json!({"resources": []})
            };
            let prompts = if capabilities.contains_key("prompts") {
                let value = client.request("prompts/list", json!({}))?;
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

    fn request(&mut self, cwd: &Path, method: &str, params: Value) -> Result<Value, String> {
        self.connect(cwd, false)?;
        let first = self
            .client
            .as_mut()
            .ok_or("MCP connection unavailable")?
            .request(method, params.clone());
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
                self.connect(cwd, false)?;
                let retry = self
                    .client
                    .as_mut()
                    .ok_or("MCP connection unavailable")?
                    .request(method, params);
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
        self.process_notifications(cwd, notifications)?;
        Ok(result)
    }

    fn process_notifications(
        &mut self,
        _cwd: &Path,
        notifications: Vec<Value>,
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
            let value = client.request("tools/list", json!({}))?;
            validate_tools(&value)?;
            self.tools = value;
        }
        if resources_changed {
            let value = client.request("resources/list", json!({}))?;
            validate_resources(&value)?;
            self.resources = value;
        }
        if prompts_changed {
            let value = client.request("prompts/list", json!({}))?;
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
