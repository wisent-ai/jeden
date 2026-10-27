//! The commands this build ships with, read from the catalogue beside it.
//!
//! They were four hundred lines of Rust literals, which meant adding a slash
//! command or a native view was a code change and a release. They are
//! `builtin-slash-commands.json` now: one row per command, its one name, and
//! whether it renders a native view. A command answers to its name only; the
//! catalogue used to declare second spellings that the dispatchers then
//! repeated, and that some of them read as different commands.

use std::sync::LazyLock;

use serde::Deserialize;

use super::{CapabilityDescriptor, CapabilityKind, FunctionTarget};

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct SlashSpec {
    pub(crate) name: String,
    pub(crate) description: String,
    /// Whether the command renders a native interactive view.
    pub(crate) view: bool,
    /// The subsystem the command drives, recorded as its descriptors'
    /// `metadata.service` the way every tool service records its own, so a
    /// probe selects the subsystem by what it declares and not by its name.
    pub(crate) service: Option<String>,
}

/// The metadata a command's descriptors carry: the service it declares.
fn spec_metadata(spec: &SlashSpec) -> Option<serde_json::Value> {
    spec.service
        .as_ref()
        .map(|service| serde_json::json!({ "service": service }))
}

static SLASH_COMMANDS: LazyLock<Vec<SlashSpec>> = LazyLock::new(|| {
    serde_json::from_str(include_str!("builtin-slash-commands.json"))
        .expect("builtin-slash-commands.json is this build s own catalogue of slash commands")
});

pub(crate) fn builtin_slash_specs() -> &'static [SlashSpec] {
    &SLASH_COMMANDS
}

pub(super) fn builtin_slash_descriptors() -> Vec<CapabilityDescriptor> {
    builtin_slash_specs()
        .iter()
        .map(|spec| {
            let descriptor = CapabilityDescriptor::new(
                format!("slash/{}", spec.name),
                CapabilityKind::SlashCommand,
                "jeden-core",
                spec.name.clone(),
                spec.description.clone(),
                FunctionTarget::BuiltinSlash {
                    command: spec.name.clone(),
                },
            )
            .operation("invoke")
            .executable(format!("/{}", spec.name));
            match spec_metadata(spec) {
                Some(metadata) => descriptor.metadata(metadata),
                None => descriptor,
            }
        })
        .collect()
}

pub(super) fn native_view_descriptors() -> Vec<CapabilityDescriptor> {
    builtin_slash_specs()
        .iter()
        .filter(|spec| spec.view)
        .map(|spec| {
            let descriptor = CapabilityDescriptor::new(
                format!("view/{}", spec.name),
                CapabilityKind::View,
                "jeden-core",
                spec.name.clone(),
                spec.description.clone(),
                FunctionTarget::NativeView {
                    command: spec.name.clone(),
                },
            )
            .dependency(format!("slash/{}", spec.name))
            .operation("render")
            .executable(format!("/{}", spec.name));
            match spec_metadata(spec) {
                Some(metadata) => descriptor.metadata(metadata),
                None => descriptor,
            }
        })
        .collect()
}

/// The file commands as capabilities. Commands that cannot be discovered
/// (installed plugins whose registry cannot be read) are one unavailable
/// service carrying the cause, as an extension discovery failure is, never
/// an empty command list.
pub(super) fn file_slash_descriptors(cwd: &std::path::Path) -> Vec<CapabilityDescriptor> {
    let commands = match crate::cli::commands::discover_file_commands(cwd) {
        Ok(commands) => commands,
        Err(error) => {
            return vec![CapabilityDescriptor::new(
                "service/file-commands",
                CapabilityKind::Service,
                "jeden-core",
                "File commands",
                "Custom slash commands from command directories and installed plugins",
                FunctionTarget::Service {
                    name: "file-commands".into(),
                },
            )
            .operation("discover")
            .health(super::CapabilityHealth::unavailable(error))]
        }
    };
    commands
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
            .policy(super::CapabilityPolicy::ReadOnly)
            .executable(format!("/{}", command.name))
            .metadata(serde_json::json!({"path": command.path}))
        })
        .collect()
}
