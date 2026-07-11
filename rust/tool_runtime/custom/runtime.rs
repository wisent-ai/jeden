use serde_json::Value;

use crate::tool_runtime::ToolRuntime;

pub(crate) fn custom_tool(
    runtime: &ToolRuntime<'_>,
    tool: &str,
    input: &Value,
) -> Result<Value, String> {
    crate::hooks::execute_extension_tool(
        runtime.cwd,
        runtime.artifact_dir,
        &runtime.operation,
        runtime.allow_write,
        runtime.allow_command,
        tool,
        input,
    )?
    .ok_or_else(|| format!("custom or extension tool not found: {tool}"))
}
