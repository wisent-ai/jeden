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

/// What a tool may do: read only, change files or durable state, or run
/// commands and code. Approval prompts, plan mode and the capability policy
/// are decided from it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ToolTier {
    Read,
    Write,
    Exec,
}

/// One builtin tool as the catalog declares it: what it is, the tier it acts
/// at, and whether its availability is probed at runtime (its descriptor then
/// comes from the probe, not from this catalog).
pub(crate) struct BuiltinTool {
    pub info: ToolInfo,
    pub tier: ToolTier,
    pub probe_backed: bool,
}

impl BuiltinTool {
    fn at(tier: ToolTier, name: &str, description: &str) -> Self {
        Self {
            info: ToolInfo::new(name, description),
            tier,
            probe_backed: false,
        }
    }
    pub(crate) fn reads(name: &str, description: &str) -> Self {
        Self::at(ToolTier::Read, name, description)
    }
    pub(crate) fn writes(name: &str, description: &str) -> Self {
        Self::at(ToolTier::Write, name, description)
    }
    pub(crate) fn runs(name: &str, description: &str) -> Self {
        Self::at(ToolTier::Exec, name, description)
    }
    pub(crate) fn probe_backed(mut self) -> Self {
        self.probe_backed = true;
        self
    }
}

/// The declared tier of the builtin tool named `name`, or nothing when no
/// builtin tool has that name.
pub(crate) fn builtin_tier(name: &str) -> Option<ToolTier> {
    builtin::built_in_tools()
        .into_iter()
        .find(|tool| tool.info.name == name)
        .map(|tool| tool.tier)
}

fn tier_policy(tier: ToolTier) -> CapabilityPolicy {
    match tier {
        ToolTier::Read => CapabilityPolicy::ReadOnly,
        ToolTier::Write | ToolTier::Exec => CapabilityPolicy::ApprovalRequired,
    }
}

pub(crate) fn builtin_capability_descriptors() -> Vec<CapabilityDescriptor> {
    let catalog = builtin::built_in_tools();
    let probed: Vec<(String, ToolTier)> = catalog
        .iter()
        .filter(|tool| tool.probe_backed)
        .map(|tool| (tool.info.name.clone(), tool.tier))
        .collect();
    let mut descriptors = catalog
        .into_iter()
        .filter(|tool| !tool.probe_backed)
        .map(
            |BuiltinTool {
                 info: tool, tier, ..
             }| {
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
                .policy(tier_policy(tier))
                .executable(tool.name)
                .metadata(json!({"input": tool.input}))
            },
        )
        .collect::<Vec<_>>();
    // A probe-backed tool is offered only while its probe reports it healthy,
    // under the tier the catalog declares for it.
    descriptors.extend(
        crate::tool_runtime::dynamic_tool_descriptors()
            .into_iter()
            .filter_map(|tool| {
                let (_, tier) = probed.iter().find(|(name, _)| *name == tool.name)?;
                Some((tool, *tier))
            })
            .map(|(tool, tier)| {
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
                .policy(tier_policy(tier))
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
    format!("{:#}\n", Value::Array(values))
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
