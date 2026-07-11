use arc_swap::ArcSwapOption;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, LazyLock, Mutex};

pub const REGISTRY_VERSION: u32 = 2;
pub const MAX_CAPABILITIES: usize = 4_096;
const MAX_ID_BYTES: usize = 256;
const MAX_OPERATIONS: usize = 64;
const MAX_DEPENDENCIES: usize = 64;
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum RegistryError {
    LockPoisoned,
}

impl std::fmt::Display for RegistryError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::LockPoisoned => formatter.write_str("capability registry rebuild lock poisoned"),
        }
    }
}

impl std::error::Error for RegistryError {}

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
    fn builtin(source: &str) -> Self {
        Self {
            provider: source.to_string(),
            artifact_digest: format!(
                "builtin:{}:{}",
                env!("CARGO_PKG_NAME"),
                env!("CARGO_PKG_VERSION")
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
    fn derived(id: &str, source: &str, target: &FunctionTarget) -> Self {
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

    fn coherent(&self) -> bool {
        !self.input_schema_id.trim().is_empty()
            && !self.output_schema_id.trim().is_empty()
            && !self.handler_id.trim().is_empty()
            && self.effective_grants.is_subset(&self.requested_grants)
    }
}

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
            version: env!("CARGO_PKG_VERSION").into(),
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

    fn normalize(&mut self) {
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
    fn valid(&self) -> bool {
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

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct ConflictDiagnostic {
    pub id: String,
    pub winner_source: String,
    pub rejected_source: String,
    pub message: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct CapabilitySnapshot {
    pub registry_version: u32,
    pub generation: u64,
    pub cwd: PathBuf,
    pub descriptors: Arc<[CapabilityDescriptor]>,
    pub diagnostics: Arc<[ConflictDiagnostic]>,
    #[serde(skip)]
    by_id: BTreeMap<String, usize>,
}

impl CapabilitySnapshot {
    fn empty() -> Self {
        Self {
            registry_version: REGISTRY_VERSION,
            generation: 0,
            cwd: PathBuf::new(),
            descriptors: Arc::from([]),
            diagnostics: Arc::from([]),
            by_id: BTreeMap::new(),
        }
    }

    pub fn get(&self, id: &str) -> Option<&CapabilityDescriptor> {
        self.by_id
            .get(id)
            .and_then(|index| self.descriptors.get(*index))
    }

    pub fn kind(&self, kind: CapabilityKind) -> impl Iterator<Item = &CapabilityDescriptor> {
        self.descriptors
            .iter()
            .filter(move |descriptor| descriptor.kind == kind)
    }

    pub fn executable_kind(
        &self,
        kind: CapabilityKind,
    ) -> impl Iterator<Item = &CapabilityDescriptor> {
        self.kind(kind)
            .filter(|descriptor| descriptor.ui.executable && descriptor.health.is_executable())
    }
}

static SNAPSHOT: LazyLock<ArcSwapOption<CapabilitySnapshot>> = LazyLock::new(ArcSwapOption::empty);
static REBUILD: LazyLock<Mutex<()>> = LazyLock::new(|| Mutex::new(()));
static DIRTY: AtomicBool = AtomicBool::new(true);

pub fn snapshot() -> Arc<CapabilitySnapshot> {
    SNAPSHOT
        .load_full()
        .unwrap_or_else(|| Arc::new(CapabilitySnapshot::empty()))
}

pub fn invalidate() {
    DIRTY.store(true, Ordering::Release);
}

fn canonical(cwd: &Path) -> PathBuf {
    cwd.canonicalize().unwrap_or_else(|_| cwd.to_path_buf())
}

pub fn for_cwd(cwd: &Path) -> Arc<CapabilitySnapshot> {
    let cwd = canonical(cwd);
    let current = snapshot();
    if !DIRTY.load(Ordering::Acquire) && current.cwd == cwd {
        return current;
    }
    refresh(&cwd).unwrap_or(current)
}

fn extend_bounded(
    target: &mut Vec<CapabilityDescriptor>,
    provider: impl IntoIterator<Item = CapabilityDescriptor>,
) {
    let remaining = MAX_CAPABILITIES.saturating_sub(target.len());
    target.extend(provider.into_iter().take(remaining));
}

pub fn refresh(cwd: &Path) -> Result<Arc<CapabilitySnapshot>, RegistryError> {
    let _guard = REBUILD.lock().map_err(|_| RegistryError::LockPoisoned)?;
    let cwd = canonical(cwd);
    let previous = snapshot();
    if !DIRTY.load(Ordering::Acquire) && previous.cwd == cwd {
        return Ok(previous);
    }
    let mut candidates = Vec::with_capacity(256);
    extend_bounded(
        &mut candidates,
        crate::tools::builtin_capability_descriptors(),
    );
    extend_bounded(
        &mut candidates,
        crate::tool_runtime::runtime_ops::capability_descriptors(&cwd),
    );
    extend_bounded(
        &mut candidates,
        crate::tool_services::capability_descriptors(&cwd),
    );
    extend_bounded(&mut candidates, builtin_slash_descriptors());
    extend_bounded(&mut candidates, native_view_descriptors());
    extend_bounded(
        &mut candidates,
        [crate::tui::external_editor_capability_descriptor(&cwd)],
    );
    extend_bounded(&mut candidates, file_slash_descriptors(&cwd));
    match crate::hooks::extension_capability_descriptors(&cwd) {
        Ok(descriptors) => extend_bounded(&mut candidates, descriptors),
        Err(error) => extend_bounded(
            &mut candidates,
            [CapabilityDescriptor::new(
                "service/extensions",
                CapabilityKind::Service,
                "extension-runtime",
                "Extensions",
                "Live extension discovery and activation",
                FunctionTarget::Service {
                    name: "extensions".into(),
                },
            )
            .operation("discover")
            .operation("refresh")
            .health(CapabilityHealth::unavailable(error))],
        ),
    }
    extend_bounded(&mut candidates, crate::mcp::capability_descriptors(&cwd));
    if candidates.len() < MAX_CAPABILITIES {
        candidates.push(
            CapabilityDescriptor::new(
                "service/capability-registry",
                CapabilityKind::Service,
                "jeden-core",
                "Capability registry",
                "Versioned atomic capability discovery and health snapshot",
                FunctionTarget::Service {
                    name: "capability-registry".into(),
                },
            )
            .operation("discover")
            .operation("refresh")
            .operation("status"),
        );
    }
    build_and_publish(cwd, previous.generation.saturating_add(1), candidates)
}

fn build_and_publish(
    cwd: PathBuf,
    generation: u64,
    candidates: Vec<CapabilityDescriptor>,
) -> Result<Arc<CapabilitySnapshot>, RegistryError> {
    let mut accepted = Vec::with_capacity(candidates.len().min(MAX_CAPABILITIES));
    let mut diagnostics = Vec::new();
    let mut by_id = BTreeMap::new();
    for mut descriptor in candidates.into_iter().take(MAX_CAPABILITIES) {
        descriptor.generation = generation;
        if descriptor.ui.executable
            && (!descriptor.binding.coherent() || !descriptor.health.is_executable())
        {
            diagnostics.push(ConflictDiagnostic {
                id: descriptor.id.clone(),
                winner_source: String::new(),
                rejected_source: descriptor.source.clone(),
                message: format!("capability '{}' declares an executable surface without coherent handler, schemas, grants, and health; descriptor rejected", descriptor.id),
            });
            continue;
        }
        if descriptor.health_checked_at == 0 {
            descriptor.health_checked_at = generation;
            descriptor.health_evidence_id =
                format!("registry:generation-{generation}:{}:health", descriptor.id);
        }
        descriptor.normalize();
        if !descriptor.valid() {
            diagnostics.push(ConflictDiagnostic {
                id: descriptor.id.clone(),
                winner_source: String::new(),
                rejected_source: descriptor.source.clone(),
                message: format!(
                    "invalid capability id '{}' from {}; descriptor rejected",
                    descriptor.id, descriptor.source
                ),
            });
            continue;
        }
        if let Some(index) = by_id.get(&descriptor.id).copied() {
            let winner: &CapabilityDescriptor = &accepted[index];
            diagnostics.push(ConflictDiagnostic {
                id: descriptor.id.clone(),
                winner_source: winner.source.clone(),
                rejected_source: descriptor.source.clone(),
                message: format!(
                    "duplicate capability id '{}': first source '{}' wins over '{}'",
                    descriptor.id, winner.source, descriptor.source
                ),
            });
            continue;
        }
        by_id.insert(descriptor.id.clone(), accepted.len());
        accepted.push(descriptor);
    }
    if accepted.len() == MAX_CAPABILITIES {
        diagnostics.push(ConflictDiagnostic {
            id: "registry/limit".into(),
            winner_source: "capability-registry".into(),
            rejected_source: "remaining providers".into(),
            message: format!(
                "capability registry reached bounded limit of {MAX_CAPABILITIES} descriptors"
            ),
        });
    }
    let built = Arc::new(CapabilitySnapshot {
        registry_version: REGISTRY_VERSION,
        generation,
        cwd,
        descriptors: Arc::from(accepted),
        diagnostics: Arc::from(diagnostics),
        by_id,
    });
    SNAPSHOT.store(Some(Arc::clone(&built)));
    DIRTY.store(false, Ordering::Release);
    Ok(built)
}

#[cfg(test)]
pub(crate) fn publish_for_test(
    cwd: &Path,
    descriptors: Vec<CapabilityDescriptor>,
) -> Result<Arc<CapabilitySnapshot>, RegistryError> {
    let generation = snapshot().generation.saturating_add(1);
    build_and_publish(canonical(cwd), generation, descriptors)
}

pub fn slash_descriptors(cwd: &Path) -> Vec<CapabilityDescriptor> {
    for_cwd(cwd)
        .kind(CapabilityKind::SlashCommand)
        .cloned()
        .collect()
}

pub fn slash_matches(cwd: &Path, prefix: &str, limit: usize) -> Vec<CapabilityDescriptor> {
    let prefix = prefix.trim_start_matches('/').to_ascii_lowercase();
    for_cwd(cwd)
        .executable_kind(CapabilityKind::SlashCommand)
        .filter(|descriptor| {
            descriptor
                .ui
                .action
                .as_deref()
                .and_then(|action| action.strip_prefix('/'))
                .is_some_and(|name| name.starts_with(&prefix))
        })
        .take(limit.min(64))
        .cloned()
        .collect()
}
pub fn view_descriptor(cwd: &Path, command: &str) -> Option<CapabilityDescriptor> {
    let command = command.trim().trim_start_matches('/').to_ascii_lowercase();
    for_cwd(cwd).get(&format!("view/{command}")).cloned()
}

pub fn is_builtin_slash(name: &str) -> bool {
    let name = name.trim().trim_start_matches('/');
    builtin_slash_specs()
        .iter()
        .any(|spec| spec.name == name || spec.aliases.contains(&name))
}

pub fn status_text(cwd: &Path) -> String {
    let snapshot = for_cwd(cwd);
    let mut by_kind = BTreeMap::<CapabilityKind, (usize, usize)>::new();
    for descriptor in snapshot.descriptors.iter() {
        let counts = by_kind.entry(descriptor.kind).or_default();
        counts.0 += 1;
        if descriptor.health.is_executable() {
            counts.1 += 1;
        }
    }
    let mut lines = vec![format!(
        "Capability registry v{} generation {}: {} descriptors, {} diagnostics",
        snapshot.registry_version,
        snapshot.generation,
        snapshot.descriptors.len(),
        snapshot.diagnostics.len()
    )];
    for (kind, (total, available)) in by_kind {
        lines.push(format!("- {:?}: {available}/{total} available", kind));
    }
    for diagnostic in snapshot.diagnostics.iter() {
        lines.push(format!("- conflict: {}", diagnostic.message));
    }
    lines.join("\n")
}

pub fn management_items(cwd: &Path) -> Vec<(String, String, String, Option<String>, bool)> {
    for_cwd(cwd)
        .descriptors
        .iter()
        .filter(|descriptor| descriptor.ui.visible)
        .map(|descriptor| {
            let available = descriptor.health.is_executable();
            let badge = if available {
                format!("{:?}", descriptor.kind).to_ascii_uppercase()
            } else {
                format!("{:?}", descriptor.health.state).to_ascii_uppercase()
            };
            let detail = descriptor
                .health
                .detail
                .clone()
                .unwrap_or_else(|| descriptor.ui.description.clone());
            let action = (available && descriptor.ui.executable)
                .then(|| descriptor.ui.action.clone())
                .flatten();
            (
                descriptor.ui.label.clone(),
                detail,
                badge,
                action,
                !available || !descriptor.ui.executable,
            )
        })
        .collect()
}

struct SlashSpec {
    name: &'static str,
    description: &'static str,
    aliases: &'static [&'static str],
}

fn builtin_slash_specs() -> &'static [SlashSpec] {
    &[
        SlashSpec {
            name: "login",
            description: "Automated OAuth login",
            aliases: &[],
        },
        SlashSpec {
            name: "logout",
            description: "Logout provider",
            aliases: &[],
        },
        SlashSpec {
            name: "refresh",
            description: "Refresh expiring Weles account",
            aliases: &[],
        },
        SlashSpec {
            name: "model",
            description: "Switch model",
            aliases: &["models", "switch"],
        },
        SlashSpec {
            name: "help",
            description: "Show slash commands",
            aliases: &["commands"],
        },
        SlashSpec {
            name: "payment-method",
            description: "Configure hosted payment method",
            aliases: &[],
        },
        SlashSpec {
            name: "billing",
            description: "Manage Weles billing policy",
            aliases: &[],
        },
        SlashSpec {
            name: "subscriptions",
            description: "Manage provider subscriptions and quota",
            aliases: &[],
        },
        SlashSpec {
            name: "mcp",
            description: "Manage MCP servers",
            aliases: &[],
        },
        SlashSpec {
            name: "settings",
            description: "Open settings menu",
            aliases: &[],
        },
        SlashSpec {
            name: "setup",
            description: "Open provider setup",
            aliases: &["providers"],
        },
        SlashSpec {
            name: "plan",
            description: "Toggle plan mode",
            aliases: &[],
        },
        SlashSpec {
            name: "plan-review",
            description: "Review latest plan",
            aliases: &[],
        },
        SlashSpec {
            name: "goal",
            description: "Toggle goal mode",
            aliases: &["guided-goal"],
        },
        SlashSpec {
            name: "loop",
            description: "Toggle loop mode",
            aliases: &[],
        },
        SlashSpec {
            name: "fast",
            description: "Toggle priority service tier",
            aliases: &[],
        },
        SlashSpec {
            name: "advisor",
            description: "Toggle advisor reviewer",
            aliases: &[],
        },
        SlashSpec {
            name: "export",
            description: "Export session",
            aliases: &[],
        },
        SlashSpec {
            name: "dump",
            description: "Dump session",
            aliases: &[],
        },
        SlashSpec {
            name: "share",
            description: "Share session",
            aliases: &[],
        },
        SlashSpec {
            name: "collab",
            description: "Collaborate via relay",
            aliases: &[],
        },
        SlashSpec {
            name: "join",
            description: "Join shared session",
            aliases: &[],
        },
        SlashSpec {
            name: "leave",
            description: "Leave collab",
            aliases: &[],
        },
        SlashSpec {
            name: "browser",
            description: "Configure browser runtime",
            aliases: &[],
        },
        SlashSpec {
            name: "copy",
            description: "Copy conversation text",
            aliases: &[],
        },
        SlashSpec {
            name: "todo",
            description: "Manage todos",
            aliases: &[],
        },
        SlashSpec {
            name: "session",
            description: "Session management",
            aliases: &[],
        },
        SlashSpec {
            name: "jobs",
            description: "Show jobs",
            aliases: &[],
        },
        SlashSpec {
            name: "usage",
            description: "Show provider usage",
            aliases: &[],
        },
        SlashSpec {
            name: "stats",
            description: "Launch stats dashboard",
            aliases: &["debug"],
        },
        SlashSpec {
            name: "status",
            description: "Show capability health",
            aliases: &[],
        },
        SlashSpec {
            name: "changelog",
            description: "Show changelog",
            aliases: &[],
        },
        SlashSpec {
            name: "hotkeys",
            description: "Show hotkeys",
            aliases: &[],
        },
        SlashSpec {
            name: "approval",
            description: "Configure tool approval policy",
            aliases: &[],
        },
        SlashSpec {
            name: "tools",
            description: "Show tools",
            aliases: &[],
        },
        SlashSpec {
            name: "context",
            description: "Show context usage",
            aliases: &[],
        },
        SlashSpec {
            name: "extensions",
            description: "Manage extensions",
            aliases: &[],
        },
        SlashSpec {
            name: "agents",
            description: "Agent controls",
            aliases: &[],
        },
        SlashSpec {
            name: "branch",
            description: "Create branch",
            aliases: &[],
        },
        SlashSpec {
            name: "fork",
            description: "Create fork",
            aliases: &[],
        },
        SlashSpec {
            name: "tree",
            description: "Navigate tree",
            aliases: &[],
        },
        SlashSpec {
            name: "ssh",
            description: "Manage SSH hosts",
            aliases: &[],
        },
        SlashSpec {
            name: "new",
            description: "Start new session",
            aliases: &["clear"],
        },
        SlashSpec {
            name: "fresh",
            description: "Reset provider stream state",
            aliases: &[],
        },
        SlashSpec {
            name: "drop",
            description: "Drop current session",
            aliases: &[],
        },
        SlashSpec {
            name: "compact",
            description: "Compact session",
            aliases: &[],
        },
        SlashSpec {
            name: "shake",
            description: "Shake session context",
            aliases: &[],
        },
        SlashSpec {
            name: "handoff",
            description: "Hand off session",
            aliases: &[],
        },
        SlashSpec {
            name: "resume",
            description: "Resume session",
            aliases: &[],
        },
        SlashSpec {
            name: "btw",
            description: "Side question",
            aliases: &[],
        },
        SlashSpec {
            name: "tan",
            description: "Background agent",
            aliases: &[],
        },
        SlashSpec {
            name: "omfg",
            description: "Forge local rule",
            aliases: &[],
        },
        SlashSpec {
            name: "retry",
            description: "Retry last failed turn",
            aliases: &[],
        },
        SlashSpec {
            name: "memory",
            description: "Memory maintenance",
            aliases: &[],
        },
        SlashSpec {
            name: "rename",
            description: "Rename session",
            aliases: &[],
        },
        SlashSpec {
            name: "move",
            description: "Move session workspace",
            aliases: &[],
        },
        SlashSpec {
            name: "marketplace",
            description: "Manage marketplace plugins",
            aliases: &[],
        },
        SlashSpec {
            name: "plugins",
            description: "Manage installed plugins",
            aliases: &[],
        },
        SlashSpec {
            name: "reload-plugins",
            description: "Reload plugins",
            aliases: &[],
        },
        SlashSpec {
            name: "hooks",
            description: "Show lifecycle hooks",
            aliases: &[],
        },
        SlashSpec {
            name: "update",
            description: "Run automated update",
            aliases: &[],
        },
        SlashSpec {
            name: "force",
            description: "Force next tool",
            aliases: &[],
        },
        SlashSpec {
            name: "exit",
            description: "Exit",
            aliases: &["quit"],
        },
    ]
}

fn builtin_slash_descriptors() -> Vec<CapabilityDescriptor> {
    builtin_slash_specs()
        .iter()
        .map(|spec| {
            CapabilityDescriptor::new(
                format!("slash/{}", spec.name),
                CapabilityKind::SlashCommand,
                "jeden-core",
                spec.name,
                spec.description,
                FunctionTarget::BuiltinSlash {
                    command: spec.name.into(),
                },
            )
            .operation("invoke")
            .executable(format!("/{}", spec.name))
        })
        .collect()
}

fn native_view_commands() -> &'static [&'static str] {
    &[
        "login",
        "logout",
        "model",
        "settings",
        "plan",
        "goal",
        "guided-goal",
        "loop",
        "fast",
        "advisor",
        "approval",
        "todo",
        "session",
        "tree",
        "branch",
        "fork",
        "new",
        "fresh",
        "drop",
        "shake",
        "resume",
        "rename",
        "move",
        "mcp",
        "ssh",
        "memory",
        "usage",
        "browser",
        "stats",
        "debug",
        "tools",
        "extensions",
        "status",
        "plugins",
        "reload-plugins",
        "marketplace",
        "jobs",
        "collab",
        "join",
        "leave",
        "share",
        "export",
        "dump",
        "copy",
        "tan",
        "omfg",
        "agents",
        "hooks",
        "changelog",
        "hotkeys",
    ]
}

fn native_view_descriptors() -> Vec<CapabilityDescriptor> {
    native_view_commands()
        .iter()
        .map(|command| {
            let description = builtin_slash_specs()
                .iter()
                .find(|spec| spec.name == *command || spec.aliases.contains(command))
                .map(|spec| spec.description)
                .unwrap_or("Native interactive command view");
            let dependency = builtin_slash_specs()
                .iter()
                .find(|spec| spec.name == *command || spec.aliases.contains(command))
                .map(|spec| spec.name)
                .unwrap_or(command);
            CapabilityDescriptor::new(
                format!("view/{command}"),
                CapabilityKind::View,
                "jeden-core",
                *command,
                description,
                FunctionTarget::NativeView {
                    command: (*command).into(),
                },
            )
            .dependency(format!("slash/{dependency}"))
            .operation("render")
            .executable(format!("/{command}"))
        })
        .collect()
}

fn file_slash_descriptors(cwd: &Path) -> Vec<CapabilityDescriptor> {
    crate::cli::commands::discover_file_commands(cwd)
        .into_iter()
        .map(|command| {
            CapabilityDescriptor::new(
                format!("slash/{}", command.name),
                CapabilityKind::SlashCommand,
                command.source.clone(),
                command.name.clone(),
                format!("File command from {}", command.source),
                FunctionTarget::FileSlash {
                    command: command.name.clone(),
                    path: command.path.clone(),
                },
            )
            .operation("expand")
            .policy(CapabilityPolicy::ReadOnly)
            .executable(format!("/{}", command.name))
            .metadata(json!({"path": command.path}))
        })
        .collect()
}

pub fn diagnostics_for(candidates: Vec<CapabilityDescriptor>) -> Vec<ConflictDiagnostic> {
    let mut seen = BTreeSet::<String>::new();
    let mut winner = BTreeMap::<String, String>::new();
    let mut diagnostics = Vec::new();
    for descriptor in candidates {
        if seen.insert(descriptor.id.clone()) {
            winner.insert(descriptor.id, descriptor.source);
        } else {
            diagnostics.push(ConflictDiagnostic {
                id: descriptor.id.clone(),
                winner_source: winner.get(&descriptor.id).cloned().unwrap_or_default(),
                rejected_source: descriptor.source.clone(),
                message: format!(
                    "duplicate capability id '{}': first source '{}' wins over '{}'",
                    descriptor.id,
                    winner.get(&descriptor.id).cloned().unwrap_or_default(),
                    descriptor.source
                ),
            });
        }
    }
    diagnostics
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn descriptor_v2_binds_handler_schemas_grants_health_and_generation() {
        let mut descriptor = CapabilityDescriptorV2::new(
            "tool/contract-fixture",
            CapabilityKind::Tool,
            "conformance-fixture",
            "Contract fixture",
            "Exercises the complete descriptor binding",
            FunctionTarget::BuiltinTool {
                name: "contract-fixture".into(),
            },
        )
        .operation("execute")
        .schemas("jeden.fixture.input.v1", "jeden.fixture.output.v1")
        .handler("fixture::execute")
        .requested_grant("fs:read")
        .effective_grant("fs:read")
        .health_evidence(42, "evidence:fixture-health")
        .executable("contract-fixture");
        descriptor.generation = 7;

        assert!(descriptor.binding.coherent());
        assert_eq!(descriptor.generation, 7);
        assert_eq!(descriptor.health_checked_at, 42);
        assert_eq!(descriptor.health_evidence_id, "evidence:fixture-health");
        let value = serde_json::to_value(&descriptor).unwrap();
        assert_eq!(value["handler_id"], "fixture::execute");
        assert_eq!(value["input_schema_id"], "jeden.fixture.input.v1");
        assert_eq!(value["output_schema_id"], "jeden.fixture.output.v1");
        assert_eq!(value["generation"], 7);
    }

    #[test]
    fn descriptor_v2_rejects_effective_grant_escalation() {
        let descriptor = CapabilityDescriptorV2::new(
            "tool/escalation-fixture",
            CapabilityKind::Tool,
            "conformance-fixture",
            "Escalation fixture",
            "Effective grant is not requested",
            FunctionTarget::BuiltinTool {
                name: "escalation-fixture".into(),
            },
        )
        .operation("execute")
        .effective_grant("network:any")
        .executable("escalation-fixture");
        assert!(!descriptor.binding.coherent());
        assert!(!descriptor.valid());
    }
}
