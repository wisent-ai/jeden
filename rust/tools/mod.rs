use serde_json::{json, Value};
use std::path::Path;

use crate::capability::{CapabilityDescriptor, CapabilityKind, CapabilityPolicy, FunctionTarget};

mod builtin;
mod mcp_tools;

pub(crate) use mcp_tools::native_mcp_tool_name;
pub use mcp_tools::native_mcp_tool_target;

#[derive(Debug, Clone)]
pub struct ToolInfo {
    pub name: String,
    pub description: String,
    pub input: Value,
}

impl ToolInfo {
    pub(crate) fn new(name: &str, description: &str) -> Self {
        Self {
            name: name.to_string(),
            description: description.to_string(),
            input: json!({}),
        }
    }
}

pub(crate) fn builtin_capability_descriptors() -> Vec<CapabilityDescriptor> {
    const PROBE_BACKED: &[&str] = &[
        "eval_session",
        "pty_session",
        "pty_resize",
        "ast_search",
        "ast_rewrite",
        "lsp",
    ];
    let mut descriptors = builtin::built_in_tools()
        .into_iter()
        .filter(|tool| !PROBE_BACKED.contains(&tool.name.as_str()))
        .map(|tool| {
            let policy = if matches!(
                tool.name.as_str(),
                "write"
                    | "write_file"
                    | "write_archive"
                    | "write_sqlite"
                    | "apply_patch"
                    | "edit_file"
                    | "edit"
                    | "delete_file"
                    | "move_file"
                    | "run_command"
                    | "run_process"
                    | "node_eval"
                    | "python_eval"
                    | "run_package_script"
            ) {
                CapabilityPolicy::ApprovalRequired
            } else {
                CapabilityPolicy::ReadOnly
            };
            CapabilityDescriptor::new(
                format!("tool/{}", tool.name),
                CapabilityKind::Tool,
                "jeden-core",
                tool.name.clone(),
                tool.description.clone(),
                FunctionTarget::BuiltinTool {
                    name: tool.name.clone(),
                },
            )
            .operation("execute")
            .policy(policy)
            .executable(tool.name)
            .metadata(json!({"input": tool.input}))
        })
        .collect::<Vec<_>>();
    descriptors.extend(
        crate::tool_runtime::dynamic_tool_descriptors()
            .into_iter()
            .filter(|tool| matches!(tool.name.as_str(), "ast_search" | "ast_rewrite" | "lsp"))
            .map(|tool| {
                let policy = if tool.name == "ast_rewrite" {
                    CapabilityPolicy::ApprovalRequired
                } else {
                    CapabilityPolicy::ReadOnly
                };
                CapabilityDescriptor::new(
                    format!("tool/{}", tool.name),
                    CapabilityKind::Tool,
                    "language-tool-runtime",
                    tool.name.clone(),
                    tool.description.clone(),
                    FunctionTarget::BuiltinTool {
                        name: tool.name.clone(),
                    },
                )
                .operation("execute")
                .policy(policy)
                .executable(tool.name)
                .metadata(json!({"input": tool.input, "health": tool.health}))
            }),
    );
    descriptors
}

pub fn list_tools(cwd: &Path) -> Vec<ToolInfo> {
    crate::capability::for_cwd(cwd)
        .executable_kind(CapabilityKind::Tool)
        .filter(|descriptor| crate::tool_runtime::tool_allowed_by_env(&descriptor.ui.label))
        .map(|descriptor| ToolInfo {
            name: descriptor.ui.label.clone(),
            description: descriptor.ui.description.clone(),
            input: descriptor
                .metadata
                .get("input")
                .cloned()
                .unwrap_or_else(|| json!({})),
        })
        .collect()
}

pub fn tools_json(cwd: &Path) -> String {
    let values = list_tools(cwd)
        .into_iter()
        .map(
            |tool| json!({"name": tool.name, "description": tool.description, "input": tool.input}),
        )
        .collect::<Vec<_>>();
    serde_json::to_string_pretty(&values).unwrap_or_else(|_| "[]".into()) + "\n"
}

pub fn tools_output(cwd: &Path, json: bool) -> String {
    if json {
        tools_json(cwd)
    } else {
        tools_table(cwd)
    }
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
    lines.extend(
        list_tools(cwd)
            .into_iter()
            .map(|tool| format!("- {}: {}", tool.name, tool.description)),
    );
    lines.join("\n")
}
