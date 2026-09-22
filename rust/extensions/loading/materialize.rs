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
    let root = cwd.join(".jeden/runtime/extensions");
    fs::create_dir_all(&root).map_err(|error| error.to_string())?;
    let stage = root.join(format!(
        "commands-{generation}.stage-{}",
        std::process::id()
    ));
    if stage.exists() {
        fs::remove_dir_all(&stage).map_err(|error| error.to_string())?;
    }
    fs::create_dir_all(&stage).map_err(|error| error.to_string())?;
    for command in commands {
        let file = stage.join(format!("{}.md", command.name));
        let contents = if command.description.trim().is_empty() {
            command.prompt.clone()
        } else {
            format!(
                "---\ndescription: {}\n---\n{}",
                serde_json::to_string(&command.description.replace('\n', " "))
                    .unwrap_or_else(|_| "\"extension command\"".into()),
                command.prompt
            )
        };
        fs::write(file, contents).map_err(|error| error.to_string())?;
    }
    let final_dir = root.join(format!("commands-{generation}"));
    if final_dir.exists() {
        fs::remove_dir_all(&final_dir).map_err(|error| error.to_string())?;
    }
    fs::rename(&stage, &final_dir).map_err(|error| error.to_string())?;
    Ok(Some(final_dir))
}
pub(super) fn materialize_agents(
    cwd: &Path,
    generation: u64,
    agents: &BTreeMap<String, declarative::Agent>,
) -> Result<Option<PathBuf>, String> {
    if agents.is_empty() {
        return Ok(None);
    }
    let root = cwd.join(".jeden/runtime/extensions");
    fs::create_dir_all(&root).map_err(|error| error.to_string())?;
    let stage = root.join(format!("agents-{generation}.stage-{}", std::process::id()));
    if stage.exists() {
        fs::remove_dir_all(&stage).map_err(|error| error.to_string())?;
    }
    fs::create_dir_all(&stage).map_err(|error| error.to_string())?;
    for agent in agents.values() {
        let file = stage.join(format!("{}.json", agent.id));
        let bytes = serde_json::to_vec_pretty(&agent.value).map_err(|error| error.to_string())?;
        fs::write(file, bytes).map_err(|error| error.to_string())?;
    }
    let final_dir = root.join(format!("agents-{generation}"));
    if final_dir.exists() {
        fs::remove_dir_all(&final_dir).map_err(|error| error.to_string())?;
    }
    fs::rename(&stage, &final_dir).map_err(|error| error.to_string())?;
    Ok(Some(final_dir))
}

pub(in crate::hooks::extensions) fn build_registry(cwd: &Path, sources: SourceSet, generation: u64) -> Result<Registry, String> {
    let files = serde_json::to_string(&sources.modules).map_err(|error| error.to_string())?;
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
    let mut extensions: Vec<HostExtension> = serde_json::from_value(
        response
            .get("extensions")
            .cloned()
            .unwrap_or_else(|| json!([])),
    )
    .map_err(|error| format!("invalid extension descriptors: {error}"))?;
    let source_set: BTreeSet<PathBuf> = sources.modules.iter().cloned().collect();
    extensions.retain(|extension| source_set.contains(&extension.source));
    for extension in &mut extensions {
        extension.precedence = sources
            .module_precedence
            .get(&extension.source)
            .copied()
            .unwrap_or_default();
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
        let mut event_indices = BTreeMap::<String, usize>::new();
        for hook in &mut extension.hooks {
            let index = event_indices.entry(hook.event.clone()).or_default();
            hook.index = *index;
            *index += 1;
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

