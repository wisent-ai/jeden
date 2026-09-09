//! Helpers the scheduler's methods stand on: where workspaces live, how an
//! agent's task context is assembled, how a child's output contract is
//! checked, and how a child process group is watched and stopped.
//!
//! Split out of `scheduler.rs` because the operator's rule keeps every source
//! file at three hundred lines or fewer, and the write guard answers an
//! oversized file by refusing every edit to it — including the one-line
//! correction of the reported isolation strategies that this split unblocked.

use super::super::types::{AgentDefinition, TaskError};
use std::fs;
use std::hash::{DefaultHasher, Hash, Hasher};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::Command;

#[cfg(unix)]
use super::kill;

/// An assembled agent context above this size is refused rather than sent.
/// One mebibyte is the ceiling the scheduler already enforced inline; it is
/// named here so the refusal and the number live in one place.
const MAX_CONTEXT_BYTES: usize = 1024 * 1024;

/// Read size when draining a child's stdout or stderr. Eight kibibytes is a
/// buffer size, not a policy: the retained volume is bounded separately by
/// the caller's `max_bytes`.
const PIPE_CHUNK_BYTES: usize = 8192;

/// Root directory that holds the isolated workspaces for jobs of a scheduler
/// with the given store and cwd. Mirrors `TaskScheduler::workspace_root` so
/// read-only tooling (e.g. `jeden worktree`) can locate managed workspaces
/// without opening (and thereby mutating) a scheduler store.
pub(crate) fn workspace_root_for(store: &Path, cwd: &Path) -> PathBuf {
    let store = fs::canonicalize(store).unwrap_or_else(|_| store.to_path_buf());
    let cwd = fs::canonicalize(cwd).unwrap_or_else(|_| cwd.to_path_buf());
    if store.starts_with(&cwd) {
        let mut hash = DefaultHasher::new();
        cwd.hash(&mut hash);
        std::env::temp_dir()
            .join("jeden-task-workspaces")
            .join(format!("{:016x}", hash.finish()))
    } else {
        store.join("workspaces")
    }
}

pub(super) fn agent_task_context(
    cwd: &Path,
    definition: &AgentDefinition,
    task: &str,
) -> Result<String, TaskError> {
    let contributions = if definition.skills.is_empty() {
        Vec::new()
    } else {
        crate::hooks::extension_skill_context(cwd, &definition.skills)
            .map_err(TaskError::Invalid)?
    };
    let mut context = String::new();
    if !definition.description.trim().is_empty() {
        context.push_str("Agent role: ");
        context.push_str(definition.description.trim());
        context.push_str("\n\n");
    }
    if !definition.prompt.trim().is_empty() {
        context.push_str("Agent instructions:\n");
        context.push_str(definition.prompt.trim());
        context.push_str("\n\n");
    }
    for skill in contributions {
        context.push_str("Declared skill ");
        context.push_str(&skill.id);
        context.push_str(":\n");
        context.push_str(&skill.content);
        if !skill.assets.is_empty() {
            context.push_str("\nValidated skill assets:\n");
            for asset in skill.assets {
                context.push_str("- ");
                context.push_str(&asset.display().to_string());
                context.push('\n');
            }
        }
        context.push('\n');
    }
    if !definition.tools.is_empty() {
        context.push_str("Hard tool allowlist: ");
        context.push_str(&definition.tools.join(", "));
        context.push_str(". Do not request any other tool.\n\n");
    }
    if !definition_output_is_unconstrained(&definition.output) {
        context.push_str("Required final JSON output schema:\n");
        context.push_str(&definition.output.to_string());
        context.push_str("\n\n");
    }
    context.push_str("Assigned task:\n");
    context.push_str(task);
    if context.len() > MAX_CONTEXT_BYTES {
        return Err(TaskError::Capacity {
            running: context.len(),
            limit: MAX_CONTEXT_BYTES,
        });
    }
    Ok(context)
}

pub(super) fn definition_output_is_unconstrained(schema: &serde_json::Value) -> bool {
    schema.is_null()
        || schema
            .as_object()
            .map(|object| object.is_empty())
            .unwrap_or(false)
}

pub(super) fn validate_output(
    value: &serde_json::Value,
    schema: &serde_json::Value,
    path: &str,
) -> Result<(), String> {
    if let Some(allowed) = schema.get("enum").and_then(serde_json::Value::as_array) {
        if !allowed.contains(value) {
            return Err(format!("{path} is not one of the allowed enum values"));
        }
    }
    let expected = schema
        .get("type")
        .and_then(serde_json::Value::as_str)
        .or_else(|| schema.as_str());
    if let Some(expected) = expected {
        let matches = match expected {
            "null" => value.is_null(),
            "boolean" => value.is_boolean(),
            "number" => value.is_number(),
            "integer" => value.as_i64().is_some() || value.as_u64().is_some(),
            "string" => value.is_string(),
            "array" => value.is_array(),
            "object" | "json" => value.is_object(),
            _ => return Err(format!("{path} uses unsupported schema type {expected}")),
        };
        if !matches {
            return Err(format!("{path} must be {expected}"));
        }
    }
    if let Some(object) = value.as_object() {
        if let Some(required) = schema.get("required").and_then(serde_json::Value::as_array) {
            for key in required.iter().filter_map(serde_json::Value::as_str) {
                if !object.contains_key(key) {
                    return Err(format!("{path}.{key} is required"));
                }
            }
        }
        if let Some(properties) = schema
            .get("properties")
            .and_then(serde_json::Value::as_object)
        {
            for (key, property_schema) in properties {
                if let Some(property) = object.get(key) {
                    validate_output(property, property_schema, &format!("{path}.{key}"))?;
                }
            }
        }
    }
    if let (Some(items), Some(item_schema)) = (value.as_array(), schema.get("items")) {
        for (index, item) in items.iter().enumerate() {
            validate_output(item, item_schema, &format!("{path}[{index}]"))?;
        }
    }
    Ok(())
}

pub(super) fn capture_pipe(
    mut pipe: impl Read,
    path: &Path,
    max_bytes: u64,
) -> Result<(), TaskError> {
    let mut file = fs::File::create(path)?;
    let mut buffer = [0u8; PIPE_CHUNK_BYTES];
    let mut written = 0u64;
    loop {
        let count = pipe.read(&mut buffer)?;
        if count == 0 {
            break;
        }
        let remaining = max_bytes.saturating_sub(written) as usize;
        let keep = remaining.min(count);
        if keep > 0 {
            file.write_all(&buffer[..keep])?;
            written += keep as u64;
        }
    }
    Ok(())
}

#[cfg(unix)]
pub(super) fn process_alive(pid: u32) -> bool {
    unsafe { kill(pid as i32, 0) == 0 }
}
#[cfg(not(unix))]
pub(super) fn process_alive(_pid: u32) -> bool {
    false
}
#[cfg(unix)]
pub(super) fn terminate_group(pid: u32, grace_ms: u64) {
    unsafe {
        kill(-(pid as i32), 15);
    }
    std::thread::sleep(std::time::Duration::from_millis(grace_ms));
    if process_alive(pid) {
        unsafe {
            kill(-(pid as i32), 9);
        }
    }
}
#[cfg(not(unix))]
pub(super) fn terminate_group(_pid: u32, _grace_ms: u64) {}
#[cfg(unix)]
pub(super) fn configure_group(command: &mut Command) {
    use std::os::unix::process::CommandExt;
    command.process_group(0);
}
#[cfg(not(unix))]
pub(super) fn configure_group(_command: &mut Command) {}
