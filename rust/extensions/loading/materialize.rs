//! Writing what the host declared to disk, and building one registry
//! generation out of it: the command and agent directories a generation owns,
//! and the registry the rest of the product reads.

use serde_json::Value;
use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use super::super::declarative;
use super::super::{CommandDescriptor, HostExtension, Registry, SourceSet};
use super::host::run_host;
use crate::hooks::extensions::ABI_VERSION;
use serde_json::json;
use std::collections::BTreeSet;

pub(super) fn materialize_commands(
    cwd: &Path,
    generation: u64,
    commands: &[CommandDescriptor],
) -> Result<Option<PathBuf>, String> {
    if commands.is_empty() {
        return Ok(None);
    }
    let files = commands
        .iter()
        .map(|command| {
            let contents = if command.description.trim().is_empty() {
                command.prompt.clone()
            } else {
                format!(
                    "---\ndescription: {}\n---\n{}",
                    serde_json::Value::String(command.description.replace('\n', " ")),
                    command.prompt
                )
            };
            (format!("{}.md", command.name), contents.into_bytes())
        })
        .collect();
    publish(cwd, "commands", generation, files).map(Some)
}

pub(super) fn materialize_agents(
    cwd: &Path,
    generation: u64,
    agents: &BTreeMap<String, declarative::Agent>,
) -> Result<Option<PathBuf>, String> {
    if agents.is_empty() {
        return Ok(None);
    }
    let files = agents
        .values()
        .map(|agent| {
            serde_json::to_vec_pretty(&agent.value)
                .map(|bytes| (format!("{}.json", agent.id), bytes))
                .map_err(|error| format!("agent {} cannot be encoded as JSON: {error}", agent.id))
        })
        .collect::<Result<Vec<_>, String>>()?;
    publish(cwd, "agents", generation, files).map(Some)
}

/// Write `files` into the directory `<kind>-<generation>` under the runtime
/// extensions directory. They are staged in a directory of this process
/// beside it and renamed into place, so a reader never sees a generation
/// half written; every failed step is refused naming its path.
fn publish(
    cwd: &Path,
    kind: &str,
    generation: u64,
    files: Vec<(String, Vec<u8>)>,
) -> Result<PathBuf, String> {
    let root = cwd.join(".jeden/runtime/extensions");
    created(&root)?;
    let stage = root.join(format!("{kind}-{generation}.stage-{}", std::process::id()));
    cleared(&stage)?;
    created(&stage)?;
    for (name, bytes) in files {
        let file = stage.join(name);
        fs::write(&file, bytes)
            .map_err(|error| format!("{} cannot be written: {error}", file.display()))?;
    }
    let final_dir = root.join(format!("{kind}-{generation}"));
    cleared(&final_dir)?;
    fs::rename(&stage, &final_dir).map_err(|error| {
        format!(
            "{} cannot be moved into place as {}: {error}",
            stage.display(),
            final_dir.display()
        )
    })?;
    Ok(final_dir)
}

fn created(directory: &Path) -> Result<(), String> {
    fs::create_dir_all(directory)
        .map_err(|error| format!("{} cannot be created: {error}", directory.display()))
}

/// Remove `directory` and what it holds; one that is not there is already
/// cleared.
fn cleared(directory: &Path) -> Result<(), String> {
    match fs::remove_dir_all(directory) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(format!(
            "{} cannot be removed: {error}",
            directory.display()
        )),
    }
}

pub(in crate::hooks::extensions) fn build_registry(
    cwd: &Path,
    sources: SourceSet,
    generation: u64,
) -> Result<Registry, String> {
    let files = serde_json::to_string(&sources.modules)
        .map_err(|error| format!("the extension module list cannot be encoded as JSON: {error}"))?;
    let response = run_host(
        cwd,
        "discover",
        generation,
        &[("JEDEN_EXTENSION_FILES", files)],
        &sources.modules,
        false,
        false,
        None,
        None,
    )?;
    if response.get("abiVersion").and_then(Value::as_u64) != Some(ABI_VERSION as u64) {
        return Err("extension host ABI mismatch".into());
    }
    let declared = response
        .get("extensions")
        .cloned()
        .ok_or("the extension host answered discover without an extensions list")?;
    let mut extensions: Vec<HostExtension> = serde_json::from_value(declared)
        .map_err(|error| format!("invalid extension descriptors: {error}"))?;
    let source_set: BTreeSet<PathBuf> = sources.modules.iter().cloned().collect();
    extensions.retain(|extension| source_set.contains(&extension.source));
    for extension in &mut extensions {
        extension.precedence = sources
            .module_precedence
            .get(&extension.source)
            .copied()
            .ok_or_else(|| {
                format!(
                    "extension module {} has no precedence in its source set",
                    extension.source.display()
                )
            })?;
    }
    extensions.sort_by(|left, right| {
        right
            .precedence
            .cmp(&left.precedence)
            .then_with(|| left.source.cmp(&right.source))
    });
    let mut tools = BTreeMap::new();
    let mut hooks = Vec::new();
    let mut commands = BTreeMap::new();
    for extension in extensions.iter_mut().filter(|extension| extension.active) {
        let mut conflict = None;
        for tool in &extension.tools {
            if tools.contains_key(&tool.name) {
                conflict = Some(format!("tool name conflict: {}", tool.name));
                break;
            }
        }
        for command in &extension.commands {
            if commands.contains_key(&command.name) {
                conflict = Some(format!("command name conflict: {}", command.name));
                break;
            }
        }
        if let Some(error) = conflict {
            extension.active = false;
            extension.health = "unhealthy".into();
            extension.error = Some(error);
            continue;
        }
        for tool in &extension.tools {
            tools.insert(tool.name.clone(), tool.clone());
        }
        for command in &extension.commands {
            commands.insert(command.name.clone(), command.clone());
        }
        // A hook's index is how many hooks of its event this extension
        // declares before it.
        let events: Vec<String> = extension
            .hooks
            .iter()
            .map(|hook| hook.event.clone())
            .collect();
        for (position, hook) in extension.hooks.iter_mut().enumerate() {
            hook.index = events[..position]
                .iter()
                .filter(|event| **event == hook.event)
                .count();
        }
        hooks.extend(extension.hooks.clone());
    }
    let declarative_inputs = sources
        .declarative
        .iter()
        .map(|capability| declarative::Input {
            kind: capability.kind,
            path: capability.path.clone(),
            precedence: capability.precedence,
        })
        .collect::<Vec<_>>();
    let declarative_runtime = declarative::load(&declarative_inputs);
    let agent_dir = materialize_agents(cwd, generation, &declarative_runtime.agents)?;
    let commands: Vec<CommandDescriptor> = commands.into_values().collect();
    let command_dir = materialize_commands(cwd, generation, &commands)?;
    Ok(Registry {
        generation,
        fingerprint: sources.fingerprint,
        extensions,
        tools,
        hooks,
        command_dir,
        declarative: sources.declarative,
        declarative_runtime,
        agent_dir,
        installed_plugins: sources.installed_plugins,
    })
}
