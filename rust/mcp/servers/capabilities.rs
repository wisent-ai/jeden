//! Reporting each configured server to the rest of the product as a
//! capability, with the health it actually has.
//!
//! Split out of `mcp/mod.rs`, which had grown past the module line cap.

use super::config::{configured_servers, string_field};
use crate::capability::{
    CapabilityDescriptor, CapabilityHealth, CapabilityKind, CapabilityPolicy, FunctionTarget,
};
use serde_json::json;
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
