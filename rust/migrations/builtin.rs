//! The migration plan of Jeden's built-in JSON documents, whose schema
//! versions are positions in `Envelope` rather than numbers written by hand.

use serde_json::Value;

use super::plan::{CompatibilityWindow, MigrationPlan, MigrationStep};

/// The schema versions of Jeden's built-in documents, oldest first: a
/// version's number is its position in this list, so a new schema is a new
/// variant at the end and a new step to it.
#[derive(Clone, Copy)]
#[repr(u32)]
pub(super) enum Envelope {
    /// Written before documents carried a version; it has no `schemaVersion`.
    Unversioned,
    /// Carries `schemaVersion`.
    Enveloped,
    /// Read within a compatibility window.
    Windowed,
}

impl Envelope {
    pub(super) const fn number(self) -> u32 {
        self as u32
    }
}

fn envelope_step(value: &mut Value) -> Result<(), String> {
    object_preflight(value)
}

static ENVELOPE_STEPS: &[MigrationStep] = &[
    MigrationStep {
        name: "version-envelope",
        from: Envelope::Unversioned.number(),
        to: Envelope::Enveloped.number(),
        apply: envelope_step,
    },
    MigrationStep {
        name: "compatibility-window-v2",
        from: Envelope::Enveloped.number(),
        to: Envelope::Windowed.number(),
        apply: envelope_step,
    },
];

pub(crate) fn builtin_document_plan(store: &'static str) -> Result<MigrationPlan, String> {
    match store {
        "session-ledger" | "task-store" | "plugin-lock" | "quality" => Ok(MigrationPlan {
            store,
            from: Envelope::Unversioned.number(),
            to: Envelope::Windowed.number(),
            reversible: true,
            preflight: object_preflight,
            steps: ENVELOPE_STEPS,
            compatibility_window: CompatibilityWindow {
                oldest_readable: Envelope::Unversioned.number(),
                newest_readable: Envelope::Windowed.number(),
                rollback_floor: Envelope::Enveloped.number(),
            },
        }),
        other => Err(format!("unknown built-in migration store: {other}")),
    }
}

pub(crate) fn object_preflight(value: &Value) -> Result<(), String> {
    match value.is_object() {
        true => Ok(()),
        false => Err("store root must be an object".into()),
    }
}
