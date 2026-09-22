//! Every extension, tool, command, hook and capability this registry holds,
//! described the way the capability registry describes anything else, with the
//! health of the extension it came from carried onto each one.

use std::path::Path;

use super::super::{CapabilityHealth, CapabilityKind, CapabilityPolicy, FunctionTarget};
use super::super::RegistryDescriptor;
use super::current;
use serde_json::json;

pub(crate) fn capability_descriptors(cwd: &Path) -> Result<Vec<RegistryDescriptor>, String> {
    let registry = current(cwd)?;
    let mut out = Vec::new();
    for extension in &registry.extensions {
        let health = if extension.active {
            CapabilityHealth::healthy()
        } else {
            CapabilityHealth::unavailable(
                extension
                    .error
                    .clone()
                    .unwrap_or_else(|| extension.health.clone()),
            )
        };
        out.push(
            RegistryDescriptor::new(
                format!(
                    "extension/{}",
                    extension.source.to_string_lossy().replace(
                        |ch: char| !ch.is_ascii_alphanumeric()
                            && ch != '-'
                            && ch != '_'
                            && ch != '.',
                        "_"
                    )
                ),
                CapabilityKind::Extension,
                extension.source.display().to_string(),
                extension
                    .source
                    .file_name()
                    .and_then(|name| name.to_str())
                    .unwrap_or("extension"),
                format!("Extension module {}", extension.source.display()),
                FunctionTarget::Extension {
                    source: extension.source.clone(),
                },
            )
            .operation("activate")
            .health(health.clone()),
        );
        for tool in &extension.tools {
            out.push(
                RegistryDescriptor::new(
                    format!("tool/{}", tool.name),
                    CapabilityKind::Tool,
                    extension.source.display().to_string(),
                    tool.name.clone(),
                    tool.description.clone(),
                    FunctionTarget::ExtensionTool {
                        name: tool.name.clone(),
                        source: extension.source.clone(),
                    },
                )
                .operation("execute")
                .policy(match tool.permission.as_deref() {
                    Some("write") | Some("command") => CapabilityPolicy::ApprovalRequired,
                    _ => CapabilityPolicy::Sandboxed,
                })
                .health(health.clone())
                .executable(tool.name.clone())
                .metadata(json!({"input": tool.input})),
            );
        }
        for command in &extension.commands {
            out.push(
                RegistryDescriptor::new(
                    format!("slash/{}", command.name),
                    CapabilityKind::SlashCommand,
                    extension.source.display().to_string(),
                    command.name.clone(),
                    command.description.clone(),
                    FunctionTarget::FileSlash {
                        command: command.name.clone(),
                        path: registry
                            .command_dir
                            .clone()
                            .unwrap_or_default()
                            .join(format!("{}.md", command.name)),
                    },
                )
                .operation("expand")
                .policy(CapabilityPolicy::Sandboxed)
                .health(health.clone())
                .executable(format!("/{}", command.name)),
            );
        }
        for declared in &extension.capabilities {
            let kind = match declared.kind.as_str() {
                "skill" | "skills" => CapabilityKind::Skill,
                "agent" | "agents" => CapabilityKind::Agent,
                "rule" | "rules" => CapabilityKind::Rule,
                "service" | "services" => CapabilityKind::Service,
                _ => CapabilityKind::PluginContribution,
            };
            let mut descriptor = RegistryDescriptor::new(
                format!("{}/{}", declared.kind.trim_end_matches('s'), declared.id),
                kind,
                extension.source.display().to_string(),
                declared.id.clone(),
                declared.description.clone(),
                FunctionTarget::Extension {
                    source: extension.source.clone(),
                },
            )
            .operation("activate")
            .health(health.clone());
            descriptor.version = declared.version.clone();
            out.push(descriptor);
        }
    }
    for plugin in &registry.installed_plugins {
        let active = registry
            .extensions
            .iter()
            .any(|extension| extension.active && extension.source.starts_with(&plugin.path))
            || registry
                .declarative_runtime
                .capabilities
                .iter()
                .any(|capability| capability.active && capability.path.starts_with(&plugin.path))
            || registry.declarative.iter().any(|capability| {
                capability.healthy
                    && matches!(capability.kind, "commands" | "hooks")
                    && capability.path.starts_with(&plugin.path)
            });
        let health = if !plugin.enabled {
            CapabilityHealth::disabled("plugin disabled by configuration")
        } else if active {
            CapabilityHealth::healthy()
        } else {
            CapabilityHealth::unavailable(
                "installed plugin has no successfully activated capabilities",
            )
        };
        let mut descriptor = RegistryDescriptor::new(
            format!("plugin/{}", plugin.id),
            CapabilityKind::PluginContribution,
            plugin.path.display().to_string(),
            plugin.id.clone(),
            "Installed plugin contribution",
            FunctionTarget::Declarative {
                path: plugin.path.clone(),
            },
        )
        .operation("discover")
        .health(health)
        .metadata(json!({"installed": true, "enabled": plugin.enabled, "active": active}));
        descriptor.version = plugin.version.clone();
        out.push(descriptor);
    }
    for capability in registry
        .declarative
        .iter()
        .filter(|capability| matches!(capability.kind, "commands" | "hooks"))
    {
        let health = if capability.healthy {
            CapabilityHealth::healthy()
        } else {
            CapabilityHealth::unavailable(
                capability
                    .error
                    .clone()
                    .unwrap_or_else(|| "declarative capability unavailable".into()),
            )
        };
        out.push(
            RegistryDescriptor::new(
                format!(
                    "contribution/{}",
                    capability.path.to_string_lossy().replace(
                        |ch: char| !ch.is_ascii_alphanumeric()
                            && ch != '-'
                            && ch != '_'
                            && ch != '.',
                        "_"
                    )
                ),
                CapabilityKind::PluginContribution,
                capability.path.display().to_string(),
                capability
                    .path
                    .file_name()
                    .and_then(|name| name.to_str())
                    .unwrap_or(capability.kind),
                format!("Activated {} contribution", capability.kind),
                FunctionTarget::Declarative {
                    path: capability.path.clone(),
                },
            )
            .operation("load")
            .policy(CapabilityPolicy::ReadOnly)
            .health(health),
        );
    }
    for capability in registry
        .declarative_runtime
        .capabilities
        .iter()
        .filter(|capability| capability.active || !capability.healthy)
    {
        let kind = match capability.kind {
            "skill" => CapabilityKind::Skill,
            "agent" => CapabilityKind::Agent,
            "rule" => CapabilityKind::Rule,
            _ => CapabilityKind::PluginContribution,
        };
        let health = if capability.healthy {
            CapabilityHealth::healthy()
        } else {
            CapabilityHealth::unavailable(
                capability
                    .error
                    .clone()
                    .unwrap_or_else(|| "definition activation failed".into()),
            )
        };
        out.push(
            RegistryDescriptor::new(
                format!("{}/{}", capability.kind, capability.id),
                kind,
                capability.path.display().to_string(),
                capability.id.clone(),
                capability.description.clone(),
                FunctionTarget::Declarative {
                    path: capability.path.clone(),
                },
            )
            .operation("load")
            .policy(CapabilityPolicy::ReadOnly)
            .health(health)
            .metadata(capability.metadata.clone()),
        );
    }
    Ok(out)
}
