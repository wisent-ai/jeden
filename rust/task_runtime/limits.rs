//! The limits delegated tasks run under, as the operator declares them in
//! `taskScheduler`: read for a run, shown and written for the CLI and Jeden
//! Desktop's Settings screen. Nothing here supplies a value; an undeclared,
//! unreadable or zero limit where one task must fit is refused by name.

use super::types::{TaskError, TaskLimits};
use serde_json::{json, Value};
use std::path::Path;

/// The config key the limits are declared under.
pub const KEY: &str = "taskScheduler";

/// The declaration, refused by name when it is absent or no task could run
/// under it.
fn checked(declared: Option<Value>) -> Result<TaskLimits, TaskError> {
    let declared = declared.filter(|value| !value.is_null()).ok_or_else(|| {
        TaskError::Invalid(format!(
            "{KEY} is not declared: state maxParallel, maxBatch, maxDepth, maxChildren and maxOutputBytes with jeden config set {KEY} '{{…}}' or on Jeden Desktop's Settings screen"
        ))
    })?;
    let limits: TaskLimits = serde_json::from_value(declared)
        .map_err(|error| TaskError::Invalid(format!("{KEY} cannot be read: {error}")))?;
    let zero = [
        ("maxParallel", limits.max_parallel),
        ("maxBatch", limits.max_batch),
        ("maxChildren", limits.max_children),
    ]
    .into_iter()
    .filter(|(_, value)| std::num::NonZeroUsize::new(*value).is_none())
    .map(|(name, _)| name)
    .chain(std::num::NonZeroU64::new(limits.max_output_bytes).is_none().then_some("maxOutputBytes"))
    .collect::<Vec<_>>();
    if !zero.is_empty() {
        return Err(TaskError::Invalid(format!(
            "{KEY} {} must be positive: no task could run under it",
            zero.join(", ")
        )));
    }
    Ok(limits)
}

/// The limits in force for `cwd`: the merged config, project layer included.
pub fn limits_from_config(cwd: &Path) -> Result<TaskLimits, TaskError> {
    checked(crate::cli::config::merged_config_value(cwd).get(KEY).cloned())
}

fn settings_json(declared: Option<Value>, path: &Path) -> Value {
    match checked(declared.clone()) {
        Ok(limits) => json!({ "limits": limits, "refusal": null, "path": path.display().to_string() }),
        Err(refusal) => json!({
            "limits": declared,
            "refusal": refusal.to_string(),
            "path": path.display().to_string(),
        }),
    }
}

/// The user-level declaration, and why it cannot be used when it cannot.
pub fn limits_settings() -> Value {
    let config = crate::cli::config::read_user_writable_config();
    settings_json(config.get(KEY).cloned(), &crate::user_config_path())
}

/// Declare `limits` at the user level, refusing a declaration no task could
/// run under before anything is written.
pub fn set_limits(limits: Value) -> Result<Value, TaskError> {
    checked(Some(limits.clone()))?;
    let mut config = crate::cli::config::read_user_writable_config();
    crate::cli::config::config_set_value(&mut config, KEY, limits.clone()).map_err(TaskError::Invalid)?;
    let path = crate::cli::config::write_user_config(&config).map_err(TaskError::Invalid)?;
    Ok(settings_json(Some(limits), &path))
}
