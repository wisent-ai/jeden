//! What a capability is made of: what kind it is, how healthy it is, what
//! policy it is subject to, what a caller sees of it, what it is bound to and
//! what function it actually reaches.

use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum CapabilityKind {
    Tool,
    SlashCommand,
    View,
    Extension,
    PluginContribution,
    Mcp,
    Skill,
    Agent,
    Rule,
    Service,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum HealthState {
    Healthy,
    Degraded,
    Unavailable,
    Disabled,
}

#[derive(Clone, Debug, Eq, PartialEq, Ord, PartialOrd, Serialize, Deserialize)]
#[serde(transparent)]
pub struct GrantId(pub String);

impl GrantId {
    pub fn new(value: impl Into<String>) -> Self {
        Self(value.into())
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct CapabilityProvenance {
    pub provider: String,
    pub artifact_digest: String,
}

impl CapabilityProvenance {
    pub(super) fn builtin(source: &str) -> Self {
        Self {
            provider: source.to_string(),
            artifact_digest: format!(
                "builtin:{}:{}",
                env!("CARGO_PKG_NAME"),
                crate::JEDEN_VERSION
            ),
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct CapabilityHealth {
    pub state: HealthState,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
}

impl CapabilityHealth {
    pub fn healthy() -> Self {
        Self {
            state: HealthState::Healthy,
            detail: None,
        }
    }

    pub fn unavailable(detail: impl Into<String>) -> Self {
        Self {
            state: HealthState::Unavailable,
            detail: Some(detail.into()),
        }
    }

    pub fn disabled(detail: impl Into<String>) -> Self {
        Self {
            state: HealthState::Disabled,
            detail: Some(detail.into()),
        }
    }

    pub fn is_executable(&self) -> bool {
        matches!(self.state, HealthState::Healthy | HealthState::Degraded)
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum CapabilityPolicy {
    ReadOnly,
    ApprovalRequired,
    Sandboxed,
    HostManaged,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct UiAffordance {
    pub label: String,
    pub description: String,
    pub visible: bool,
    pub executable: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub action: Option<String>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "kebab-case")]
pub enum FunctionTarget {
    BuiltinTool {
        name: String,
    },
    ExtensionTool {
        name: String,
        source: PathBuf,
    },
    McpTool {
        native_name: String,
        server: String,
        remote_name: String,
    },
    BuiltinSlash {
        command: String,
    },
    FileSlash {
        command: String,
        path: PathBuf,
    },
    NativeView {
        command: String,
    },
    Extension {
        source: PathBuf,
    },
    Declarative {
        path: PathBuf,
    },
    McpServer {
        name: String,
    },
    Service {
        name: String,
    },
    None,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct CapabilityBinding {
    pub input_schema_id: String,
    pub output_schema_id: String,
    pub handler_id: String,
    pub requested_grants: BTreeSet<GrantId>,
    pub effective_grants: BTreeSet<GrantId>,
}

impl CapabilityBinding {
    pub(super) fn derived(id: &str, source: &str, target: &FunctionTarget) -> Self {
        let handler_id = match target {
            FunctionTarget::None => String::new(),
            _ => format!("{source}::{id}"),
        };
        Self {
            input_schema_id: format!("jeden.capability.{id}.input.v1"),
            output_schema_id: format!("jeden.capability.{id}.output.v1"),
            handler_id,
            requested_grants: BTreeSet::new(),
            effective_grants: BTreeSet::new(),
        }
    }

    pub(super) fn coherent(&self) -> bool {
        !self.input_schema_id.trim().is_empty()
            && !self.output_schema_id.trim().is_empty()
            && !self.handler_id.trim().is_empty()
            && self.effective_grants.is_subset(&self.requested_grants)
    }
}

