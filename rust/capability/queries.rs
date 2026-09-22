//! What a caller asks the registry: which slash commands there are, which of
//! them match what is being typed, which view answers a command, and the lines
//! the management screen shows.

use std::collections::BTreeMap;
use std::path::Path;

use super::builtin::builtin_slash_specs;
use super::registry::for_cwd;
use super::shapes::CapabilityKind;
use super::CapabilityDescriptor;

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
        .any(|spec| spec.name == name || spec.aliases.iter().any(|alias| alias == name))
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

pub fn status_json(cwd: &Path) -> Result<String, String> {
    serde_json::to_string_pretty(for_cwd(cwd).as_ref()).map_err(|error| error.to_string())
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
