//! Checking that what a server returned is the shape the protocol promises,
//! before any of it reaches a model.
//!
//! Split out of `mcp/mod.rs`, which had grown past the module line cap.

use serde_json::Value;

pub(super) fn validate_tools(value: &Value) -> Result<(), String> {
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

pub(super) fn validate_resources(value: &Value) -> Result<(), String> {
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

pub(super) fn validate_prompts(value: &Value) -> Result<(), String> {
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
