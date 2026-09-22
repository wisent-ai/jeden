//! One capability described: everything the registry, the console and a caller
//! are told about it, and the checks that decide whether it is fit to publish.

use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::shapes::{
    CapabilityBinding, CapabilityHealth, CapabilityKind, CapabilityPolicy, CapabilityProvenance,
    FunctionTarget, GrantId, UiAffordance,
};
use super::{MAX_DEPENDENCIES, MAX_ID_BYTES, MAX_OPERATIONS};

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct CapabilityDescriptorV2 {
    pub id: String,
    pub kind: CapabilityKind,
    pub source: String,
    pub version: String,
    pub operations: Vec<String>,
    pub provenance: CapabilityProvenance,
    pub dependencies: Vec<String>,
    pub health: CapabilityHealth,
    pub policy: CapabilityPolicy,
    pub ui: UiAffordance,
    pub target: FunctionTarget,
    #[serde(flatten)]
    pub binding: CapabilityBinding,
    pub generation: u64,
    pub health_checked_at: u64,
    pub health_evidence_id: String,
    #[serde(default, skip_serializing_if = "Value::is_null")]
    pub metadata: Value,
}

impl CapabilityDescriptorV2 {
    pub fn new(
        id: impl Into<String>,
        kind: CapabilityKind,
        source: impl Into<String>,
        label: impl Into<String>,
        description: impl Into<String>,
        target: FunctionTarget,
    ) -> Self {
        let id = id.into();
        let source = source.into();
        let binding = CapabilityBinding::derived(&id, &source, &target);
        let label = label.into();
        Self {
            id,
            kind,
            provenance: CapabilityProvenance::builtin(&source),
            source,
            version: crate::JEDEN_VERSION.into(),
            operations: Vec::new(),
            dependencies: Vec::new(),
            health: CapabilityHealth::healthy(),
            policy: CapabilityPolicy::HostManaged,
            ui: UiAffordance {
                label,
                description: description.into(),
                visible: true,
                executable: false,
                action: None,
            },
            target,
            binding,
            health_checked_at: 0,
            health_evidence_id: "builtin-constructor-health".into(),
            generation: 0,
            metadata: Value::Null,
        }
    }

    pub fn executable(mut self, action: impl Into<String>) -> Self {
        self.ui.executable = true;
        self.ui.action = Some(action.into());
        self
    }

    pub fn operation(mut self, operation: impl Into<String>) -> Self {
        self.operations.push(operation.into());
        self
    }

    pub fn dependency(mut self, dependency: impl Into<String>) -> Self {
        self.dependencies.push(dependency.into());
        self
    }

    pub fn policy(mut self, policy: CapabilityPolicy) -> Self {
        self.policy = policy;
        self
    }

    pub fn health(mut self, health: CapabilityHealth) -> Self {
        self.health = health;
        self
    }

    pub fn health_evidence(mut self, checked_at: u64, evidence_id: impl Into<String>) -> Self {
        self.health_checked_at = checked_at;
        self.health_evidence_id = evidence_id.into();
        self
    }

    pub fn schemas(
        mut self,
        input_schema_id: impl Into<String>,
        output_schema_id: impl Into<String>,
    ) -> Self {
        self.binding.input_schema_id = input_schema_id.into();
        self.binding.output_schema_id = output_schema_id.into();
        self
    }

    pub fn handler(mut self, handler_id: impl Into<String>) -> Self {
        self.binding.handler_id = handler_id.into();
        self
    }

    pub fn requested_grant(mut self, grant: impl Into<String>) -> Self {
        self.binding.requested_grants.insert(GrantId::new(grant));
        self
    }

    pub fn effective_grant(mut self, grant: impl Into<String>) -> Self {
        self.binding.effective_grants.insert(GrantId::new(grant));
        self
    }

    pub fn provenance(
        mut self,
        provider: impl Into<String>,
        artifact_digest: impl Into<String>,
    ) -> Self {
        self.provenance = CapabilityProvenance {
            provider: provider.into(),
            artifact_digest: artifact_digest.into(),
        };
        self
    }

    pub fn metadata(mut self, metadata: Value) -> Self {
        self.metadata = metadata;
        self
    }

    pub(super) fn normalize(&mut self) {
        self.operations.truncate(MAX_OPERATIONS);
        self.dependencies.truncate(MAX_DEPENDENCIES);
        if !self.health.is_executable() {
            self.ui.executable = false;
            self.ui.action = None;
        }
        if self.ui.executable && self.ui.action.as_deref().is_none_or(str::is_empty) {
            self.ui.executable = false;
            self.ui.action = None;
        }
        if self.ui.executable && !self.binding.coherent() {
            self.ui.executable = false;
            self.ui.action = None;
        }
    }
    pub(super) fn valid(&self) -> bool {
        !self.id.is_empty()
            && self.id.len() <= MAX_ID_BYTES
            && self
                .id
                .chars()
                .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '/' | ':' | '-' | '_' | '.'))
            && !self.provenance.provider.trim().is_empty()
            && !self.provenance.artifact_digest.trim().is_empty()
            && !self.health_evidence_id.trim().is_empty()
            && (!self.ui.executable || (self.binding.coherent() && self.health.is_executable()))
    }
}

pub type CapabilityDescriptor = CapabilityDescriptorV2;
