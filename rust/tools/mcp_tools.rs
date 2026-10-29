use serde_json::Value;
use std::path::Path;

/// The prefix every native name of an MCP tool starts with.
const NATIVE_PREFIX: &str = "mcp__";

pub(crate) fn native_mcp_tool_name(server_name: &str, tool_name: &str) -> String {
    fn safe(value: &str) -> String {
        value
            .chars()
            .map(|ch| {
                if ch.is_ascii_alphanumeric() || ch == '_' {
                    ch
                } else {
                    '_'
                }
            })
            .collect()
    }
    format!("{NATIVE_PREFIX}{}__{}", safe(server_name), safe(tool_name))
}

/// The server and remote name of the MCP tool `native_name` names: none for
/// a name that is no MCP tool's, and for an MCP name the live servers do
/// not offer; the live servers that cannot be read are refused, never read
/// as offering nothing.
pub fn native_mcp_tool_target(
    cwd: &Path,
    native_name: &str,
) -> Result<Option<(String, String)>, String> {
    if !native_name.starts_with(NATIVE_PREFIX) {
        return Ok(None);
    }
    Ok(crate::mcp::live_tools(cwd)?
        .into_iter()
        .find_map(|(server_name, tool)| {
            let raw_name = tool.get("name").and_then(Value::as_str)?;
            (native_mcp_tool_name(&server_name, raw_name) == native_name)
                .then(|| (server_name, raw_name.to_string()))
        }))
}
