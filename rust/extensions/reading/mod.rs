//! The registry this process serves from: the generation currently loaded for
//! one working directory, the reload that replaces it, and the retirement of
//! the generated directories the previous generation owned.

use std::path::Path;
use std::sync::Arc;

use super::{Registry, ReloadReport, REGISTRIES};
use super::loading::materialize::build_registry;
use super::loading::roots::source_set;
use super::canonical_key;

pub(super) mod capabilities;
pub(super) mod entries;

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

pub(super) fn current(cwd: &Path) -> Result<Arc<Registry>, String> {
    let key = canonical_key(cwd);
    let sources = source_set(cwd)?;
    let previous = REGISTRIES
        .read()
        .map_err(|_| "extension registry lock poisoned")?
        .get(&key)
        .cloned();
    if let Some(registry) = &previous {
        if registry.fingerprint == sources.fingerprint {
            return Ok(registry.clone());
        }
    }
    let generation = previous
        .as_ref()
        .map(|registry| registry.generation.saturating_add(1))
        .unwrap_or(1);
    let previous_command_registry = previous.clone();
    let built = match build_registry(cwd, sources, generation) {
        Ok(registry) => Arc::new(registry),
        Err(_) if previous.is_some() => return Ok(previous.expect("checked previous registry")),
        Err(error) => return Err(error),
    };
    REGISTRIES
        .write()
        .map_err(|_| "extension registry lock poisoned")?
        .insert(key, built.clone());
    retire_generated_dirs(previous_command_registry.as_ref(), &built);
    Ok(built)
}

pub fn reload(cwd: &Path) -> Result<ReloadReport, String> {
    let key = canonical_key(cwd);
    let sources = source_set(cwd)?;
    let previous = REGISTRIES
        .read()
        .map_err(|_| "extension registry lock poisoned")?
        .get(&key)
        .cloned();
    let generation = previous
        .as_ref()
        .map(|registry| registry.generation.saturating_add(1))
        .unwrap_or(1);
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
            .filter(|capability| {
                capability.healthy && matches!(capability.kind, "commands" | "hooks")
            })
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
    REGISTRIES
        .write()
        .map_err(|_| "extension registry lock poisoned")?
        .insert(key, built.clone());
    retire_generated_dirs(previous.as_ref(), &built);
    crate::capability::invalidate();
    Ok(report)
}
