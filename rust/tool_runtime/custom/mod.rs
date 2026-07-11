use serde_json::Value;

use crate::tool_runtime::shared::{object_input, string_input, u64_input};
use crate::tool_runtime::ToolRuntime;

mod runtime;

pub(crate) use runtime::custom_tool;

pub(crate) fn mcp_native_tool(
    runtime: &ToolRuntime<'_>,
    tool: &str,
    input: &Value,
) -> Option<Result<Value, String>> {
    crate::tools::native_mcp_tool_target(runtime.cwd, tool).map(|(server, native_tool)| {
        crate::mcp::call_tool(runtime.cwd, &server, &native_tool, input.clone(), 30_000)
    })
}

fn mcp_timeout_ms(input: &Value) -> u64 {
    u64_input(input, "timeoutMs", 30_000).clamp(1_000, 120_000)
}

fn mcp_server(input: &Value) -> Result<String, String> {
    string_input(input, "server")
        .filter(|server| !server.is_empty())
        .ok_or_else(|| "server is required".into())
}

pub(crate) fn mcp_list_tools(runtime: &ToolRuntime<'_>, input: &Value) -> Result<Value, String> {
    let server = mcp_server(input)?;
    crate::mcp::list_tools(runtime.cwd, &server, mcp_timeout_ms(input))
}

pub(crate) fn mcp_call_tool(runtime: &ToolRuntime<'_>, input: &Value) -> Result<Value, String> {
    let server = mcp_server(input)?;
    let tool = string_input(input, "tool")
        .filter(|tool| !tool.is_empty())
        .ok_or_else(|| "tool is required".to_string())?;
    crate::mcp::call_tool(
        runtime.cwd,
        &server,
        &tool,
        object_input(input, "args"),
        mcp_timeout_ms(input),
    )
}

pub(crate) fn mcp_list_resources(
    runtime: &ToolRuntime<'_>,
    input: &Value,
) -> Result<Value, String> {
    let server = mcp_server(input)?;
    crate::mcp::list_resources(runtime.cwd, &server, mcp_timeout_ms(input))
}

pub(crate) fn mcp_read_resource(runtime: &ToolRuntime<'_>, input: &Value) -> Result<Value, String> {
    let server = mcp_server(input)?;
    let uri = string_input(input, "uri")
        .filter(|uri| !uri.is_empty())
        .ok_or_else(|| "uri is required".to_string())?;
    crate::mcp::read_resource(runtime.cwd, &server, &uri, mcp_timeout_ms(input))
}

pub(crate) fn mcp_list_prompts(runtime: &ToolRuntime<'_>, input: &Value) -> Result<Value, String> {
    let server = mcp_server(input)?;
    crate::mcp::list_prompts(runtime.cwd, &server, mcp_timeout_ms(input))
}

pub(crate) fn mcp_get_prompt(runtime: &ToolRuntime<'_>, input: &Value) -> Result<Value, String> {
    let server = mcp_server(input)?;
    let name = string_input(input, "name")
        .filter(|name| !name.is_empty())
        .ok_or_else(|| "name is required".to_string())?;
    crate::mcp::get_prompt(
        runtime.cwd,
        &server,
        &name,
        object_input(input, "args"),
        mcp_timeout_ms(input),
    )
}
