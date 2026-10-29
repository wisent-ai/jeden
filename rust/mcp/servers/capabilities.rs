//! Reporting each configured server to the rest of the product as a
//! capability, with the health it actually has.
//!
//! Split out of `mcp/mod.rs`, which had grown past the module line cap.

use crate::capability::{
    CapabilityDescriptor, CapabilityHealth, CapabilityKind, CapabilityPolicy, FunctionTarget,
};
use crate::mcp::connection::ConnectionState;
use crate::mcp::managers;
use crate::mcp::session_key;
use crate::mcp::sweep::live_tools;
use serde_json::json;
use serde_json::Value;
use std::path::Path;

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
    let discovery_error = live_tools(cwd).err();
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
        // A server that is not ready says why when it recorded an error, and
        // otherwise the state it stands in, which is then the whole cause.
        let health = match (ready, connection.failures.last().cloned()) {
            (true, _) => CapabilityHealth::healthy(),
            (false, Some(error)) => CapabilityHealth::unavailable(error),
            (false, None) => CapabilityHealth::unavailable(format!(
                "MCP server is {}",
                connection.state.as_str()
            )),
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
            // MCP makes a tool's description optional; one the server leaves
            // out is named by the tool and its server, never left blank.
            let description = match tool.get("description").and_then(Value::as_str) {
                Some(description) => description.to_string(),
                None => format!("MCP tool {remote_name} from {server}"),
            };
            // tools/list is validated to give every tool an object input
            // schema; one without it is a tool nobody can call correctly.
            let (input, tool_health) = match tool.get("inputSchema") {
                Some(schema) => (schema.clone(), health.clone()),
                None => (
                    Value::Null,
                    CapabilityHealth::unavailable(format!(
                        "MCP tool {remote_name} from {server} declares no inputSchema"
                    )),
                ),
            };
            out.push(
                CapabilityDescriptor::new(
                    format!("tool/{native_name}"),
                    CapabilityKind::Tool,
                    format!("mcp:{server}"),
                    native_name.clone(),
                    description,
                    FunctionTarget::McpTool {
                        native_name: native_name.clone(),
                        server: server.clone(),
                        remote_name: remote_name.into(),
                    },
                )
                .operation("execute")
                .dependency(format!("mcp/{server}"))
                .policy(CapabilityPolicy::Sandboxed)
                .health(tool_health)
                .executable(native_name)
                .metadata(json!({"input": input})),
            );
        }
    }
    out
}
