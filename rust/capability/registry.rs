//! The snapshot this process serves from, and how it is rebuilt: one
//! generation for one working directory, published whole, with the conflicts
//! that decided which source won each identifier.

use arc_swap::ArcSwapOption;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, LazyLock, Mutex};

use super::builtin::{builtin_slash_descriptors, file_slash_descriptors, native_view_descriptors};
use super::shapes::CapabilityKind;
use super::{CapabilityDescriptor, RegistryError, MAX_CAPABILITIES, REGISTRY_VERSION};
use crate::capability::shapes::CapabilityHealth;
use crate::capability::shapes::FunctionTarget;
use std::collections::BTreeMap;

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
    pub(super) fn empty() -> Self {
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
    extend_bounded(
        &mut candidates,
        crate::tui::attachment_capability_descriptors(&cwd),
    );
    extend_bounded(
        &mut candidates,
        [crate::tui::keymap_capability_descriptor()],
    );
    extend_bounded(
        &mut candidates,
        crate::roadmap::capability_descriptors(&cwd),
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

