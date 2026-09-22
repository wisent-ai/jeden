//! One connection to an external tool server: starting it, asking it things,
//! and ending it.

use reqwest::blocking::Client as HttpClient;
use serde_json::{json, Value};
use std::collections::VecDeque;
use std::path::Path;
use std::process::Child;
use std::sync::mpsc::Receiver;

use super::servers::string_field;
use super::MCP_PROTOCOL_VERSION;
use std::sync::mpsc;

mod exchange;
mod framing;
mod launch;

const MAX_NOTIFICATIONS: usize = 256;
const MAX_SESSION_ID_BYTES: usize = 1024;
const MCP_SESSION_ID: &str = "mcp-session-id";

pub(super) struct StdioTransport {
    pub(super) child: Child,
    pub(super) responses: Receiver<Result<Value, String>>,
    pub(super) stderr: Receiver<String>,
}

pub(super) struct HttpTransport {
    pub(super) client: HttpClient,
    pub(super) url: String,
    pub(super) session_id: Option<String>,
}

pub(super) enum Transport {
    Stdio(StdioTransport),
    Http(HttpTransport),
}

pub(crate) struct McpClient {
    pub(super) transport: Transport,
    pub(super) next_id: u64,
    pub(super) notifications: VecDeque<Value>,
}

impl McpClient {
    pub(super) fn start(server: &Value, cwd: &Path) -> Result<Self, String> {
        let sandbox = crate::tool_runtime::runtime_ops::SecureRuntime::detect()
            .health()
            .clone();
        if !sandbox.enforced() {
            return Err(format!(
                "enforced sandbox unavailable: {}: {}",
                sandbox.backend, sandbox.detail
            ));
        }
        let object = server
            .as_object()
            .ok_or("MCP server config must be an object")?;
        let transport_name = string_field(server, "type").unwrap_or_else(|| {
            if object.contains_key("url") {
                "http"
            } else {
                "stdio"
            }
        });
        let transport = match transport_name {
            "stdio" => Transport::Stdio(Self::start_stdio(server, cwd)?),
            "http" | "streamable-http" => Transport::Http(Self::start_http(server)?),
            other => {
                return Err(format!(
                    "unsupported MCP transport '{other}'; expected stdio or streamable-http"
                ))
            }
        };
        Ok(Self {
            transport,
            next_id: 1,
            notifications: VecDeque::new(),
        })
    }

    pub(super) fn request(&mut self, method: &str, params: Value) -> Result<Value, String> {
        let id = self.next_id;
        self.next_id = self
            .next_id
            .checked_add(1)
            .ok_or("MCP request id exhausted")?;
        let message = json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params});
        loop {
            let messages = self.send(&message)?;
            for response in messages {
                if response.get("method").is_some()
                    && response.get("id").is_none()
                    && self.notifications.len() >= MAX_NOTIFICATIONS
                {
                    return Err("MCP notification queue limit exceeded".into());
                }
                if response.get("jsonrpc").and_then(Value::as_str) != Some("2.0") {
                    return Err("MCP response has invalid jsonrpc version".into());
                }
                if response.get("method").is_some() && response.get("id").is_none() {
                    self.notifications.push_back(response);
                    continue;
                }
                if response.get("id").and_then(Value::as_u64) != Some(id) {
                    continue;
                }
                if let Some(error) = response.get("error") {
                    return Err(error
                        .get("message")
                        .and_then(Value::as_str)
                        .map(ToOwned::to_owned)
                        .unwrap_or_else(|| error.to_string()));
                }
                return response
                    .get("result")
                    .cloned()
                    .ok_or("MCP response is missing result".into());
            }
            if matches!(self.transport, Transport::Http(_)) {
                return Err("MCP HTTP response did not contain the matching request id".into());
            }
        }
    }

    fn notify(&mut self, method: &str, params: Value) -> Result<(), String> {
        let message = json!({"jsonrpc": "2.0", "method": method, "params": params});
        for response in self.send(&message)? {
            if response.get("method").is_some() && response.get("id").is_none() {
                if self.notifications.len() >= MAX_NOTIFICATIONS {
                    return Err("MCP notification queue limit exceeded".into());
                }
                self.notifications.push_back(response);
            }
        }
        Ok(())
    }

    pub(super) fn initialize(&mut self) -> Result<Value, String> {
        let init = self.request(
            "initialize",
            json!({
                "protocolVersion": MCP_PROTOCOL_VERSION,
                "capabilities": {},
                "clientInfo": {"name": "jeden", "version": crate::JEDEN_VERSION},
            }),
        )?;
        if !init.is_object()
            || init
                .get("protocolVersion")
                .and_then(Value::as_str)
                .is_none()
            || !init
                .get("capabilities")
                .map(Value::is_object)
                .unwrap_or(false)
        {
            return Err("MCP initialize result has invalid schema".into());
        }
        self.notify("notifications/initialized", json!({}))?;
        Ok(init)
    }

    pub(super) fn take_notifications(&mut self) -> Vec<Value> {
        self.notifications.drain(..).collect()
    }

    pub(super) fn poll_notifications(&mut self) -> Result<Vec<Value>, String> {
        if let Transport::Stdio(transport) = &mut self.transport {
            loop {
                match transport.responses.try_recv() {
                    Ok(Ok(message))
                        if message.get("method").is_some() && message.get("id").is_none() =>
                    {
                        self.notifications.push_back(message)
                    }
                    Ok(Ok(_)) => {
                        return Err(
                            "MCP server sent an unexpected response without an active request"
                                .into(),
                        )
                    }
                    Ok(Err(error)) => return Err(error),
                    Err(mpsc::TryRecvError::Empty) => break,
                    Err(mpsc::TryRecvError::Disconnected) => {
                        return Err("MCP stdio transport closed".into())
                    }
                }
            }
        }
        Ok(self.notifications.drain(..).collect())
    }

    pub(super) fn is_alive(&mut self) -> bool {
        match &mut self.transport {
            Transport::Stdio(transport) => matches!(transport.child.try_wait(), Ok(None)),
            Transport::Http(_) => true,
        }
    }

    pub(super) fn close(&mut self) {
        match &mut self.transport {
            Transport::Stdio(transport) => {
                drop(transport.child.stdin.take());
                if matches!(transport.child.try_wait(), Ok(None)) {
                    let _ = transport.child.kill();
                }
                let _ = transport.child.wait();
                let _ = transport.stderr.try_recv();
            }
            Transport::Http(transport) => {
                if let Some(session_id) = transport.session_id.take() {
                    let _ = transport
                        .client
                        .delete(&transport.url)
                        .header(MCP_SESSION_ID, session_id)
                        .send();
                }
            }
        }
    }
}

impl Drop for McpClient {
    fn drop(&mut self) {
        self.close();
    }
}
