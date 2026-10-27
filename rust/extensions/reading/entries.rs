//! What the rest of the product reads off a loaded registry: the command and
//! agent directories, the models an extension declares, and the providers.

use std::path::{Path, PathBuf};

use super::current;
use crate::hooks::extensions::declarative;
use crate::hooks::extensions::loading::host::run_host;
use serde_json::Value;

pub(crate) fn command_dirs(cwd: &Path) -> Result<Vec<PathBuf>, String> {
    Ok(current(cwd)?.command_dir.iter().cloned().collect())
}
pub(crate) fn agent_dirs(cwd: &Path) -> Result<Vec<PathBuf>, String> {
    Ok(current(cwd)?.agent_dir.iter().cloned().collect())
}

/// What the active extensions of the registry loaded for `cwd` declare,
/// `pick`ed from each in precedence order. A registry that cannot be read is
/// refused with its cause: read as no extensions, the models and providers
/// they declare would vanish from the catalog with nothing said.
fn declared<T: Clone>(cwd: &Path, pick: impl Fn(&super::super::HostExtension) -> &[T]) -> Result<Vec<T>, String> {
    let registry = current(cwd).map_err(|cause| format!("the extension registry cannot be read: {cause}"))?;
    let mut extensions = registry
        .extensions
        .iter()
        .filter(|extension| extension.active)
        .collect::<Vec<_>>();
    extensions.sort_by(|left, right| {
        left.precedence
            .cmp(&right.precedence)
            .then_with(|| left.source.cmp(&right.source))
    });
    Ok(extensions.into_iter().flat_map(|extension| pick(extension).iter().cloned()).collect())
}

pub(crate) fn model_entries(cwd: &Path) -> Result<Vec<crate::control_plane::brama::ModelEntry>, String> {
    declared(cwd, |extension| &extension.models)
}

pub fn provider_entries(cwd: &Path) -> Result<Vec<crate::control_plane::weles::Provider>, String> {
    declared(cwd, |extension| &extension.providers)
}
pub(crate) fn prompt_context(
    cwd: &Path,
    prompt: &str,
) -> Result<Vec<declarative::PromptContribution>, String> {
    Ok(declarative::prompt_context(
        &current(cwd)?.declarative_runtime,
        prompt,
    ))
}

pub(crate) fn skill_context(
    cwd: &Path,
    skill_ids: &[String],
) -> Result<Vec<declarative::PromptContribution>, String> {
    declarative::skill_context(&current(cwd)?.declarative_runtime, skill_ids)
}

pub(crate) fn execute_tool(
    cwd: &Path,
    artifact_dir: Option<&Path>,
    operation: &crate::tool_runtime::runtime_ops::OperationContext<'_>,
    allow_write: bool,
    allow_command: bool,
    name: &str,
    input: &Value,
) -> Result<Option<Value>, String> {
    let registry = current(cwd)?;
    let Some(tool) = registry.tools.get(name) else {
        return Ok(None);
    };
    let response = run_host(
        cwd,
        "execute_tool",
        registry.generation,
        &[
            (
                "JEDEN_EXTENSION_SOURCE",
                tool.source.to_string_lossy().into_owned(),
            ),
            ("JEDEN_EXTENSION_TARGET", name.to_string()),
            ("JEDEN_EXTENSION_INPUT", input.to_string()),
        ],
        std::slice::from_ref(&tool.source),
        allow_write,
        allow_command,
        artifact_dir,
        Some(operation),
    )?;
    // host.mjs answers a tool's own return value as `result`; a tool that
    // returned nothing (undefined) leaves the key out of the JSON, which is
    // the tool's own empty answer, null.
    Ok(Some(match response.get("result") {
        Some(result) => result.clone(),
        None => Value::Null,
    }))
}

pub(crate) fn fire_hooks(
    cwd: &Path,
    event: &str,
    tool: &str,
    payload: &Value,
    allow_command: bool,
) -> Result<Vec<Value>, String> {
    let registry = current(cwd)?;
    let mut matching = Vec::new();
    for hook in registry.hooks.iter().filter(|hook| hook.event == event) {
        let applies = tool.is_empty()
            || hook.matcher.is_empty()
            || regex::Regex::new(&hook.matcher)
                .map_err(|error| {
                    format!(
                        "the {event} hook #{} of {} has a tool matcher that is no regular expression ({}): {error}",
                        hook.index,
                        hook.source.display(),
                        hook.matcher
                    )
                })?
                .is_match(tool);
        if applies {
            matching.push(hook);
        }
    }
    let mut results = Vec::new();
    for hook in matching {
        let index = hook.index;
        let response = run_host(
            cwd,
            "fire_hook",
            registry.generation,
            &[
                (
                    "JEDEN_EXTENSION_SOURCE",
                    hook.source.to_string_lossy().into_owned(),
                ),
                ("JEDEN_EXTENSION_EVENT", event.to_string()),
                ("JEDEN_EXTENSION_HOOK_INDEX", index.to_string()),
                ("JEDEN_EXTENSION_INPUT", payload.to_string()),
            ],
            std::slice::from_ref(&hook.source),
            false,
            allow_command,
            None,
            None,
        )?;
        // host.mjs always answers a hook with `result` (null for one that
        // returned nothing); a reply without it is not the host's.
        let result = response.get("result").cloned().ok_or_else(|| {
            format!(
                "the extension host answered the {event} hook #{index} of {} without its result: {response}",
                hook.source.display()
            )
        })?;
        results.push(result);
    }
    Ok(results)
}

/// Whether any loaded extension registers a hook on `event`. An unreadable
/// registry counts as registered: `fire_hooks` would report that failure as
/// an outcome of the event, so a caller deciding what to hold back until the
/// event has run must assume it will run.
pub(crate) fn has_hooks(cwd: &Path, event: &str) -> bool {
    match current(cwd) {
        Ok(registry) => registry.hooks.iter().any(|hook| hook.event == event),
        Err(_unread) => true,
    }
}
