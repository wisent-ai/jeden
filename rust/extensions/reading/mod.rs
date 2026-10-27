//! The registry this process serves from: the generation currently loaded for
//! one working directory, the reload that replaces it, and the retirement of
//! the generated directories the previous generation owned.

use std::path::Path;
use std::sync::Arc;

use super::canonical_key;
use super::loading::materialize::build_registry;
use super::loading::roots::source_set;
use super::{Registry, ReloadReport, REGISTRIES};
use std::fs;

pub(crate) mod capabilities;
pub(crate) mod entries;

pub(super) fn retire_generated_dirs(previous: Option<&Arc<Registry>>, current: &Registry) {
    let Some(previous) = previous else {
        return;
    };
    for (label, path, retained) in [
        (
            "command",
            previous.command_dir.as_ref(),
            current.command_dir.as_ref(),
        ),
        (
            "agent",
            previous.agent_dir.as_ref(),
            current.agent_dir.as_ref(),
        ),
    ] {
        let Some(path) = path else {
            continue;
        };
        if retained == Some(path) {
            continue;
        }
        if let Err(error) = fs::remove_dir_all(path) {
            if error.kind() != std::io::ErrorKind::NotFound {
                eprintln!(
                    "failed to retire extension {label} generation {}: {error}",
                    path.display()
                );
            }
        }
    }
}

/// The generation a rebuilt registry takes: the one after the registry it
/// replaces, or the first when none was loaded.
fn next_generation(previous: Option<&Arc<Registry>>) -> u64 {
    match previous {
        Some(registry) => registry
            .generation
            .saturating_add(u64::from(crate::tally::STEP)),
        None => crate::tally::FIRST,
    }
}

pub(super) fn current(cwd: &Path) -> Result<Arc<Registry>, String> {
    let key = canonical_key(cwd);
    let sources = source_set(cwd)?;
    // Registries are inserted whole per workspace: a holder that panicked left
    // a map every later load can still read.
    let previous = crate::panicked::read_held(&REGISTRIES).get(&key).cloned();
    if let Some(registry) = &previous {
        if registry.fingerprint == sources.fingerprint {
            return Ok(registry.clone());
        }
    }
    let generation = next_generation(previous.as_ref());
    let previous_command_registry = previous.clone();
    // The extension sources changed; a rebuild that fails is the answer, not
    // the registry built from the sources as they were before.
    let built = Arc::new(build_registry(cwd, sources, generation).map_err(|error| {
        format!("the extensions changed and could not be loaded again: {error}")
    })?);
    crate::panicked::written(&REGISTRIES).insert(key, built.clone());
    retire_generated_dirs(previous_command_registry.as_ref(), &built);
    Ok(built)
}

pub fn reload(cwd: &Path) -> Result<ReloadReport, String> {
    let key = canonical_key(cwd);
    let sources = source_set(cwd)?;
    let previous = crate::panicked::read_held(&REGISTRIES).get(&key).cloned();
    let generation = next_generation(previous.as_ref());
    let built = Arc::new(build_registry(cwd, sources, generation)?);
    let commands = built
        .extensions
        .iter()
        .filter(|extension| extension.active)
        .map(|extension| extension.commands.len())
        .sum();
    let capabilities = built
        .extensions
        .iter()
        .filter(|extension| extension.active)
        .map(|extension| extension.capabilities.len())
        .sum::<usize>()
        + built
            .declarative
            .iter()
            .filter(|capability| capability.healthy && capability.carried)
            .count()
        + built
            .declarative_runtime
            .capabilities
            .iter()
            .filter(|capability| capability.active)
            .count();
    let report = ReloadReport {
        generation,
        active_extensions: built
            .extensions
            .iter()
            .filter(|extension| extension.active)
            .count(),
        unhealthy_extensions: built
            .extensions
            .iter()
            .filter(|extension| !extension.active)
            .count(),
        tools: built.tools.len(),
        commands,
        hooks: built.hooks.len(),
        capabilities,
    };
    crate::panicked::written(&REGISTRIES).insert(key, built.clone());
    retire_generated_dirs(previous.as_ref(), &built);
    crate::capability::invalidate();
    Ok(report)
}
