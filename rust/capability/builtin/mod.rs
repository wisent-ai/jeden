//! The commands this build ships with, read from the catalogues beside it.
//!
//! They were four hundred lines of Rust literals, which meant adding a slash
//! command or a native view was a code change and a release. They are
//! `builtin-slash-commands.json` and `native-view-commands.json` now: one row
//! per command, with the aliases each answers to.

use std::sync::LazyLock;

use serde::Deserialize;

use super::{CapabilityDescriptor, CapabilityKind, FunctionTarget};

#[derive(Deserialize)]
pub(crate) struct SlashSpec {
    pub(crate) name: String,
    pub(crate) description: String,
    #[serde(default)]
    pub(crate) aliases: Vec<String>,
}

static SLASH_COMMANDS: LazyLock<Vec<SlashSpec>> = LazyLock::new(|| {
    serde_json::from_str(include_str!("builtin-slash-commands.json"))
        .expect("builtin-slash-commands.json is this build s own catalogue of slash commands")
});

static VIEW_COMMANDS: LazyLock<Vec<String>> = LazyLock::new(|| {
    serde_json::from_str(include_str!("native-view-commands.json"))
        .expect("native-view-commands.json is this build s own list of native views")
});

pub(crate) fn builtin_slash_specs() -> &'static [SlashSpec] {
    &SLASH_COMMANDS
}

pub(super) fn native_view_commands() -> &'static [String] {
    &VIEW_COMMANDS
}

pub(super) fn builtin_slash_descriptors() -> Vec<CapabilityDescriptor> {
    builtin_slash_specs()
        .iter()
        .map(|spec| {
            CapabilityDescriptor::new(
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
            .executable(format!("/{}", spec.name))
        })
        .collect()
}

pub(super) fn native_view_descriptors() -> Vec<CapabilityDescriptor> {
    native_view_commands()
        .iter()
        .map(|command| {
            let named = builtin_slash_specs()
                .iter()
                .find(|spec| &spec.name == command || spec.aliases.contains(command));
            let description = named
                .map(|spec| spec.description.clone())
                .unwrap_or_else(|| "Native interactive command view".to_string());
            let dependency = named
                .map(|spec| spec.name.clone())
                .unwrap_or_else(|| command.clone());
            CapabilityDescriptor::new(
                format!("view/{command}"),
                CapabilityKind::View,
                "jeden-core",
                command.clone(),
                description,
                FunctionTarget::NativeView {
                    command: command.clone(),
                },
            )
            .dependency(format!("slash/{dependency}"))
            .operation("render")
            .executable(format!("/{command}"))
        })
        .collect()
}

pub(super) fn file_slash_descriptors(cwd: &std::path::Path) -> Vec<CapabilityDescriptor> {
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
            .policy(super::CapabilityPolicy::ReadOnly)
            .executable(format!("/{}", command.name))
            .metadata(serde_json::json!({"path": command.path}))
        })
        .collect()
}
