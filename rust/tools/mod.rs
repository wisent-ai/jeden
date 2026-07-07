use serde_json::{json, Value};
use std::collections::BTreeSet;
use std::path::Path;

mod builtin;
mod custom;
mod mcp_tools;

pub use mcp_tools::native_mcp_tool_target;

#[derive(Debug, Clone)]
pub struct ToolInfo {
    pub name: String,
    pub description: String,
    pub input: Value,
}

impl ToolInfo {
    fn new(name: &str, description: &str) -> Self {
        Self { name: name.to_string(), description: description.to_string(), input: json!({}) }
    }
}

pub fn list_tools(cwd: &Path) -> Vec<ToolInfo> {
    let mut seen = BTreeSet::new();
    let mut tools = builtin::built_in_tools();
    for tool in &tools { seen.insert(tool.name.clone()); }
    tools.extend(custom::static_custom_tools(cwd, &mut seen));
    tools.extend(mcp_tools::static_mcp_tools(cwd, &mut seen));
    tools
}

pub fn tools_json(cwd: &Path) -> String {
    let values = list_tools(cwd)
        .into_iter()
        .map(|tool| json!({"name": tool.name, "description": tool.description, "input": tool.input}))
        .collect::<Vec<_>>();
    serde_json::to_string_pretty(&values).unwrap_or_else(|_| "[]".into()) + "\n"
}

pub fn tools_output(cwd: &Path, json: bool) -> String {
    if json { tools_json(cwd) } else { tools_table(cwd) }
}

pub fn tools_table(cwd: &Path) -> String {
    let mut out = String::new();
    for tool in list_tools(cwd) {
        out.push_str(&tool.name);
        out.push('\t');
        out.push_str(&tool.description);
        out.push('\n');
    }
    out
}

pub fn tools_slash_text(cwd: &Path) -> String {
    let mut lines = vec!["Tools visible to Jeden:".to_string()];
    lines.extend(list_tools(cwd).into_iter().map(|tool| format!("- {}: {}", tool.name, tool.description)));
    lines.join("\n")
}
