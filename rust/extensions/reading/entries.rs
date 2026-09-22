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

pub(crate) fn model_entries(cwd: &Path) -> Vec<crate::control_plane::brama::ModelEntry> {
    let registry = match current(cwd) {
        Ok(registry) => registry,
        Err(_) => {
            return Vec::new();
        }
    };
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
    extensions
        .into_iter()
        .flat_map(|extension| extension.models.clone())
        .collect()
}

pub fn provider_entries(cwd: &Path) -> Vec<crate::control_plane::weles::Provider> {
    let registry = match current(cwd) {
        Ok(registry) => registry,
        Err(_) => {
            return Vec::new();
        }
    };
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
    extensions
        .into_iter()
        .flat_map(|extension| extension.providers.clone())
        .collect()
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
    Ok(Some(response.get("result").cloned().unwrap_or(Value::Null)))
}

pub(crate) fn fire_hooks(
    cwd: &Path,
    event: &str,
    tool: &str,
    payload: &Value,
    allow_command: bool,
) -> Result<Vec<Value>, String> {
    let registry = current(cwd)?;
    let mut results = Vec::new();
    for hook in registry.hooks.iter().filter(|hook| {
        hook.event == event
            && (tool.is_empty()
                || hook.matcher.is_empty()
                || regex::Regex::new(&hook.matcher)
                    .map(|matcher| matcher.is_match(tool))
                    .unwrap_or(false))
    }) {
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
        results.push(response.get("result").cloned().unwrap_or(Value::Null));
    }
    Ok(results)
}
