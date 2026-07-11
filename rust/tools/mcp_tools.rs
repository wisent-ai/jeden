use serde_json::Value;
use std::path::Path;

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
    format!("mcp__{}__{}", safe(server_name), safe(tool_name))
}

pub fn native_mcp_tool_target(cwd: &Path, native_name: &str) -> Option<(String, String)> {
    crate::mcp::live_tools(cwd, 30_000)
        .ok()?
        .into_iter()
        .find_map(|(server_name, tool)| {
            let raw_name = tool.get("name").and_then(Value::as_str)?;
            (native_mcp_tool_name(&server_name, raw_name) == native_name)
                .then(|| (server_name, raw_name.to_string()))
        })
}
