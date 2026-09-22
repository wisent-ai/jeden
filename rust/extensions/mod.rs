mod declarative;

use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, LazyLock, RwLock};

use crate::capability::{
    CapabilityDescriptor as RegistryDescriptor, CapabilityHealth, CapabilityKind, CapabilityPolicy,
    FunctionTarget,
};

const ABI_VERSION: u32 = 1;

const MAX_EXTENSION_FILES: usize = 256;
const MAX_DESCRIPTOR_BYTES: usize = 2 * 1024 * 1024;
const HOST: &str = include_str!("host.mjs");

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ToolDescriptor {
    pub name: String,
    pub description: String,
    #[serde(default)]
    pub input: Value,
    #[serde(default)]
    pub permission: Option<String>,
    pub source: PathBuf,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub(crate) struct CommandDescriptor {
    pub name: String,
    #[serde(default)]
    pub description: String,
    pub prompt: String,
    pub source: PathBuf,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub(crate) struct HookDescriptor {
    pub event: String,
    #[serde(default)]
    pub matcher: String,
    pub source: PathBuf,
    #[serde(default)]
    pub index: usize,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub(crate) struct CapabilityDescriptor {
    pub id: String,
    pub kind: String,
    pub version: String,
    #[serde(default)]
    pub description: String,
}

#[derive(Clone, Debug, Deserialize)]
struct HostExtension {
    source: PathBuf,
    active: bool,
    health: String,
    #[serde(default)]
    error: Option<String>,
    #[serde(default)]
    tools: Vec<ToolDescriptor>,
    #[serde(default)]
    commands: Vec<CommandDescriptor>,
    #[serde(default)]
    hooks: Vec<HookDescriptor>,
    #[serde(default)]
    capabilities: Vec<CapabilityDescriptor>,
    #[serde(default)]
    providers: Vec<crate::control_plane::weles::Provider>,
    #[serde(default)]
    models: Vec<crate::control_plane::brama::ModelEntry>,
    #[serde(skip)]
    precedence: usize,
}

#[derive(Clone, Debug)]
struct DeclarativeCapability {
    kind: &'static str,
    path: PathBuf,
    healthy: bool,
    error: Option<String>,
    precedence: usize,
}

#[derive(Clone, Debug)]
struct InstalledPluginRoot {
    id: String,
    version: String,
    path: PathBuf,
    enabled: bool,
}

#[derive(Clone, Debug)]
struct SourceSet {
    modules: Vec<PathBuf>,
    declarative: Vec<DeclarativeCapability>,
    installed_plugins: Vec<InstalledPluginRoot>,
    module_precedence: BTreeMap<PathBuf, usize>,
    fingerprint: u64,
}

#[derive(Clone, Debug)]
struct Registry {
    generation: u64,
    fingerprint: u64,
    extensions: Vec<HostExtension>,
    tools: BTreeMap<String, ToolDescriptor>,
    hooks: Vec<HookDescriptor>,
    command_dir: Option<PathBuf>,
    declarative: Vec<DeclarativeCapability>,
    declarative_runtime: declarative::Loaded,
    agent_dir: Option<PathBuf>,
    installed_plugins: Vec<InstalledPluginRoot>,
}

#[derive(Clone, Debug)]
pub struct ReloadReport {
    pub generation: u64,
    pub active_extensions: usize,
    pub unhealthy_extensions: usize,
    pub tools: usize,
    pub commands: usize,
    pub hooks: usize,
    pub capabilities: usize,
}

static REGISTRIES: LazyLock<RwLock<BTreeMap<PathBuf, Arc<Registry>>>> =
    LazyLock::new(|| RwLock::new(BTreeMap::new()));


pub(crate) mod loading;
mod reading;

pub use reading::reload;
pub(crate) use reading::capabilities::capability_descriptors;
pub(crate) use reading::entries::{
    agent_dirs, command_dirs, execute_tool, model_entries, prompt_context, skill_context,
};
pub use reading::entries::provider_entries;

use loading::canonical_key;
use reading::current;
pub(crate) use crate::hooks::extensions::reading::entries::fire_hooks;


pub fn status(cwd: &Path) -> Result<String, String> {
    let registry = current(cwd)?;
    let mut lines = vec![format!(
        "Extension registry ABI {} generation {}: {} active, {} unhealthy",
        ABI_VERSION,
        registry.generation,
        registry
            .extensions
            .iter()
            .filter(|extension| extension.active)
            .count(),
        registry
            .extensions
            .iter()
            .filter(|extension| !extension.active)
            .count()
    )];
    for extension in &registry.extensions {
        lines.push(format!(
            "- {} [{}] tools={} commands={} hooks={} providers={} models={}{}",
            extension.source.display(),
            if extension.active {
                "active"
            } else {
                &extension.health
            },
            extension.tools.len(),
            extension.commands.len(),
            extension.hooks.len(),
            extension.providers.len(),
            extension.models.len(),
            extension
                .error
                .as_ref()
                .map(|error| format!(": {error}"))
                .unwrap_or_default()
        ));
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
        lines.push(format!(
            "- plugin {} version={} installed=yes enabled={} active={}",
            plugin.id, plugin.version, plugin.enabled, active
        ));
    }
    for capability in registry
        .declarative
        .iter()
        .filter(|capability| matches!(capability.kind, "commands" | "hooks"))
    {
        lines.push(format!(
            "- {} {} [{}]{}",
            capability.kind,
            capability.path.display(),
            if capability.healthy {
                "active"
            } else {
                "unhealthy"
            },
            capability
                .error
                .as_ref()
                .map(|error| format!(": {error}"))
                .unwrap_or_default()
        ));
    }
    for capability in &registry.declarative_runtime.capabilities {
        lines.push(format!(
            "- {} {} ({}) [{}]{}",
            capability.kind,
            capability.id,
            capability.path.display(),
            if capability.active {
                "active"
            } else if capability.healthy {
                "shadowed"
            } else {
                "unhealthy"
            },
            capability
                .error
                .as_ref()
                .map(|error| format!(": {error}"))
                .unwrap_or_default()
        ));
    }
    Ok(lines.join("\n"))
}
