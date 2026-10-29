//! What this installation can do, as one registry the whole product reads.
//!
//! A capability is described once and answered for everywhere: the shapes it
//! is made of, the descriptor that carries it, the snapshot one working
//! directory is served from, the questions callers ask of it, and the commands
//! this build ships with.

/// The capability registry document version this build publishes
/// (https://github.com/wisent-ai/jeden/blob/main/README.md#capability-registry).
pub const REGISTRY_VERSION: u32 = 2;
/// The generation of the empty registry published before the first rebuild,
/// which no published rebuild carries
/// (https://github.com/wisent-ai/jeden/blob/main/README.md#capability-registry).
pub(crate) const EMPTY_GENERATION: u64 = 0;
/// The step each published rebuild adds to the generation
/// (https://github.com/wisent-ai/jeden/blob/main/README.md#capability-registry).
pub(crate) const GENERATION_STEP: u64 = 1;

mod builtin;
mod descriptor;
mod queries;
mod registry;
mod shapes;

pub use descriptor::CapabilityDescriptorV2;
pub type CapabilityDescriptor = CapabilityDescriptorV2;
pub(crate) use builtin::builtin_slash_specs;
pub use queries::{
    diagnostics_for, is_builtin_slash, management_items, slash_descriptors, slash_matches,
    status_json, status_text, view_descriptor,
};
pub use registry::{
    for_cwd, invalidate, refresh, snapshot, CapabilitySnapshot, ConflictDiagnostic,
};
pub use shapes::{
    CapabilityBinding, CapabilityHealth, CapabilityKind, CapabilityPolicy, CapabilityProvenance,
    FunctionTarget, GrantId, HealthState, UiAffordance,
};
