//! One server's connection, and the state it is in.
//!
//! Split out of `mcp/mod.rs`, which had grown past the module line cap.

use super::client::McpClient;
use crate::mcp::validate::validate_prompts;
use crate::mcp::validate::validate_resources;
use crate::mcp::validate::validate_tools;
use serde_json::{json, Value};
use std::path::Path;

/// Where one server's connection stands. A failed start or exchange is
/// reported with its count and last error; the next request tries again,
/// because no wait or failure count here would be anything but a guess.
#[derive(Clone, Copy)]
pub(crate) enum ConnectionState {
    Disconnected,
    Connecting,
    Ready,
    Failed,
}

impl ConnectionState {
    pub(super) fn as_str(self) -> &'static str {
        match self {
            Self::Disconnected => "disconnected",
            Self::Connecting => "connecting",
            Self::Ready => "ready",
            Self::Failed => "failed",
        }
    }
}

pub(super) struct ServerConnection {
    pub(super) config: Value,
    pub(super) client: Option<McpClient>,
    pub(super) state: ConnectionState,
    pub(super) failures: u32,
    pub(super) last_error: Option<String>,
    pub(super) initialize: Value,
    pub(super) tools: Value,
    pub(super) resources: Value,
    pub(super) prompts: Value,
}

impl ServerConnection {
    pub(super) fn new(config: Value) -> Self {
        Self {
            config,
            client: None,
            state: ConnectionState::Disconnected,
            failures: 0,
            last_error: None,
            initialize: Value::Null,
            tools: json!({"tools": []}),
            resources: json!({"resources": []}),
            prompts: json!({"prompts": []}),
        }
    }

    pub(super) fn disconnect(&mut self) {
        if let Some(mut client) = self.client.take() {
            client.close();
        }
        self.state = ConnectionState::Disconnected;
    }

    pub(super) fn record_failure(&mut self, error: String) {
        self.disconnect();
        self.failures = self.failures.saturating_add(1);
        self.last_error = Some(error);
        self.state = ConnectionState::Failed;
    }

    pub(super) fn connect(&mut self, cwd: &Path, force: bool) -> Result<(), String> {
        if !force && !self.transport_broken() {
            return Ok(());
        }
        self.disconnect();
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
                self.last_error = None;
                Ok(())
            }
            Err(error) => {
                self.record_failure(error.clone());
                Err(error)
            }
        }
    }

    pub(super) fn request(
        &mut self,
        cwd: &Path,
        method: &str,
        params: Value,
    ) -> Result<Value, String> {
        self.connect(cwd, false)?;
        let first = self
            .client
            .as_mut()
            .ok_or("MCP connection unavailable")?
            .request(method, params.clone());
        let result = match first {
            Ok(value) => Ok(value),
            Err(error) => {
                if !self.transport_broken() {
                    return Err(error);
                }
                self.record_failure(error);
                self.connect(cwd, false)?;
                let retry = self
                    .client
                    .as_mut()
                    .ok_or("MCP connection unavailable")?
                    .request(method, params);
                if retry.is_err() && self.transport_broken() {
                    if let Err(error) = &retry {
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

    /// The client's process exited or its last exchange failed at the
    /// transport; a server's own error answer is neither.
    fn transport_broken(&mut self) -> bool {
        self.client
            .as_mut()
            .map(|client| !client.is_alive() || client.transport_failed())
            .unwrap_or(true)
    }

    pub(super) fn process_notifications(
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
