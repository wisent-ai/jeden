//! What this installation can do, as one registry the whole product reads.
//!
//! A capability is described once and answered for everywhere: the shapes it
//! is made of, the descriptor that carries it, the snapshot one working
//! directory is served from, the questions callers ask of it, and the commands
//! this build ships with.

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


mod builtin;
mod descriptor;
mod queries;
mod registry;
mod shapes;

pub use descriptor::CapabilityDescriptorV2;
pub type CapabilityDescriptor = CapabilityDescriptorV2;
pub use queries::{
    diagnostics_for, is_builtin_slash, management_items, slash_descriptors, slash_matches,
    status_json, status_text, view_descriptor,
};
pub use registry::{for_cwd, invalidate, refresh, snapshot, CapabilitySnapshot, ConflictDiagnostic};
pub use shapes::{
    CapabilityBinding, CapabilityHealth, CapabilityKind, CapabilityPolicy, CapabilityProvenance,
    FunctionTarget, GrantId, HealthState, UiAffordance,
};
pub(crate) use builtin::{builtin_slash_specs, SlashSpec};
