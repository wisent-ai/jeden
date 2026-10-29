mod builtin;
mod journal;
mod plan;

pub(crate) use builtin::{builtin_document_plan, object_preflight};
pub(crate) use plan::{CompatibilityWindow, MigrationPlan, MigrationStep};

use builtin::Envelope;

use crate::tool_runtime::runtime_ops::platform::presence::{present, remove_present};
use journal::{journal_path, sync_parent, write_durable, JournalStage, MigrationJournal};
use serde_json::{json, Value};
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum MigrationOutcome {
    Current {
        version: u32,
    },
    Migrated {
        from: u32,
        to: u32,
        backup: Option<PathBuf>,
    },
    Recovered {
        version: u32,
    },
}

/// The key every migrated document carries its schema version under.
pub(crate) const SCHEMA_VERSION_KEY: &str = "schemaVersion";

fn version(value: &Value) -> Result<u32, String> {
    match value.get(SCHEMA_VERSION_KEY) {
        None => Ok(Envelope::Unversioned.number()),
        Some(found) => {
            let number = found.as_u64().ok_or_else(|| {
                format!("schemaVersion must be a non-negative integer, not {found}")
            })?;
            u32::try_from(number)
                .map_err(|error| format!("schemaVersion {number} is not a 32-bit integer: {error}"))
        }
    }
}

fn refuse_unsupported(plan: &MigrationPlan, found: u32) -> Result<(), String> {
    if found > plan.to {
        return Err(format!(
            "{} schema version {} is newer than supported version {}",
            plan.store, found, plan.to
        ));
    }
    if found < plan.compatibility_window.oldest_readable {
        return Err(format!(
            "{} schema version {} is older than compatibility floor {}",
            plan.store, found, plan.compatibility_window.oldest_readable
        ));
    }
    Ok(())
}

pub(crate) fn recover_json(
    path: &Path,
    plan: &MigrationPlan,
) -> Result<Option<MigrationOutcome>, String> {
    let journal_file = journal_path(path);
    let Some(state) = journal::read(&journal_file)? else {
        return Ok(None);
    };
    if state.target != path || state.store != plan.store || state.to != plan.to {
        return Err(format!(
            "migration journal {} does not match requested store",
            journal_file.display()
        ));
    }
    match state.stage {
        JournalStage::Prepared | JournalStage::BackupDurable => {
            remove_present(&state.staged, "staged migration")?;
            if !present(path)? {
                let backup = match &state.backup {
                    Some(backup) if present(backup)? => backup,
                    _absent => {
                        return Err(format!(
                            "migration recovery has neither old store nor backup for {}",
                            path.display()
                        ))
                    }
                };
                fs::copy(backup, path)
                    .map_err(|error| format!("restore {}: {error}", path.display()))?;
                sync_file(path)?;
            }
        }
        JournalStage::Committed => {
            let bytes = fs::read(path)
                .map_err(|error| format!("read committed {}: {error}", path.display()))?;
            let value: Value = serde_json::from_slice(&bytes)
                .map_err(|error| format!("invalid committed {}: {error}", path.display()))?;
            if version(&value)? != plan.to {
                return Err(format!(
                    "committed {} does not contain schema version {}",
                    path.display(),
                    plan.to
                ));
            }
            remove_present(&state.staged, "staged migration")?;
        }
    }
    fs::remove_file(&journal_file)
        .map_err(|error| format!("remove journal {}: {error}", journal_file.display()))?;
    sync_parent(path)?;
    let recovered =
        fs::read(path).map_err(|error| format!("read recovered {}: {error}", path.display()))?;
    let value: Value = serde_json::from_slice(&recovered)
        .map_err(|error| format!("invalid recovered {}: {error}", path.display()))?;
    Ok(Some(MigrationOutcome::Recovered {
        version: version(&value)?,
    }))
}

pub(crate) fn migrate_json(path: &Path, plan: &MigrationPlan) -> Result<MigrationOutcome, String> {
    plan.validate()?;
    if let Some(outcome) = recover_json(path, plan)? {
        return Ok(outcome);
    }
    let original = fs::read(path).map_err(|error| format!("read {}: {error}", path.display()))?;
    // A store that is neither JSON nor YAML is refused with both causes: the
    // JSON one was dropped, and a JSON store with one stray byte was said to
    // be bad YAML.
    let mut value: Value = serde_json::from_slice(&original).or_else(|json_error| {
        serde_yaml::from_slice(&original).map_err(|yaml_error| {
            format!(
                "corrupt {} store {}: as JSON, {json_error}; as YAML, {yaml_error}",
                plan.store,
                path.display()
            )
        })
    })?;
    (plan.preflight)(&value)?;
    let from = version(&value)?;
    refuse_unsupported(plan, from)?;
    if from == plan.to {
        return Ok(MigrationOutcome::Current { version: from });
    }
    if from < plan.from {
        return Err(format!(
            "{} has no migration path from schema version {}",
            plan.store, from
        ));
    }
    let mut current = from;
    for step in plan.steps.iter().filter(|step| step.from >= from) {
        if step.from != current {
            return Err(format!(
                "{} migration gap at version {}",
                plan.store, current
            ));
        }
        (step.apply)(&mut value).map_err(|error| {
            format!(
                "{} migration step {} failed: {error}",
                plan.store, step.name
            )
        })?;
        current = step.to;
        value["schemaVersion"] = json!(current);
    }
    if current != plan.to {
        return Err(format!(
            "{} migration stopped at {}, expected {}",
            plan.store, current, plan.to
        ));
    }
    (plan.preflight)(&value)?;

    let staged = sibling(path, "migration-stage")?;
    let backup = path.with_extension(format!("backup-v{from}"));
    write_synced(
        &staged,
        &serde_json::to_vec_pretty(&value)
            .map_err(|error| format!("encode migrated {}: {error}", path.display()))?,
    )?;
    let journal_file = journal_path(path);
    let mut state = MigrationJournal {
        store: plan.store.into(),
        from,
        to: plan.to,
        target: path.into(),
        staged: staged.clone(),
        backup: Some(backup.clone()),
        stage: JournalStage::Prepared,
    };
    write_durable(&journal_file, &state)?;
    write_synced(&backup, &original)?;
    state.stage = JournalStage::BackupDurable;
    write_durable(&journal_file, &state)?;
    fs::rename(&staged, path)
        .map_err(|error| format!("atomic commit {}: {error}", path.display()))?;
    sync_parent(path)?;
    state.stage = JournalStage::Committed;
    write_durable(&journal_file, &state)?;
    fs::remove_file(&journal_file)
        .map_err(|error| format!("remove journal {}: {error}", journal_file.display()))?;
    sync_parent(path)?;
    Ok(MigrationOutcome::Migrated {
        from,
        to: plan.to,
        backup: Some(backup),
    })
}

pub(crate) fn write_json_atomic(path: &Path, value: &Value) -> Result<(), String> {
    write_bytes_atomic(
        path,
        &serde_json::to_vec_pretty(value)
            .map_err(|error| format!("encode {}: {error}", path.display()))?,
    )
}

/// Write `text` to `path` through the shared durable replace, so a reader
/// sees either the old file or the whole new one. The text is written as
/// is; callers own the trailing newline.
pub(crate) fn write_text_atomic(path: &Path, text: &str) -> Result<(), String> {
    write_bytes_atomic(path, text.trim_end_matches('\n').as_bytes())
}

/// `bytes` and one closing newline as the whole content of `path`. These
/// writers rewrite files the user owns (the user config, the contract block
/// in a project's agent instructions), so an existing file keeps its mode
/// and a new one is created private.
fn write_bytes_atomic(path: &Path, bytes: &[u8]) -> Result<(), String> {
    let content = [bytes, b"\n".as_slice()].concat();
    crate::tool_runtime::runtime_ops::durable::replace_keeping_mode(path, &content)
}

/// A hidden staging file beside `path`, named after it byte for byte; a
/// path that names no file has no sibling and is refused.
fn sibling(path: &Path, suffix: &str) -> Result<PathBuf, String> {
    let name = path
        .file_name()
        .ok_or_else(|| format!("{} names no file to stage beside", path.display()))?;
    let mut staged = std::ffi::OsString::from(".");
    staged.push(name);
    staged.push(format!(".{suffix}-{}", std::process::id()));
    Ok(path.with_file_name(staged))
}

fn write_synced(path: &Path, bytes: &[u8]) -> Result<(), String> {
    let mut file = OpenOptions::new()
        .create(true)
        .truncate(true)
        .write(true)
        .open(path)
        .map_err(|error| format!("create {}: {error}", path.display()))?;
    file.write_all(bytes)
        .and_then(|()| file.write_all(b"\n"))
        .map_err(|error| format!("write {}: {error}", path.display()))?;
    file.sync_all()
        .map_err(|error| format!("fsync {}: {error}", path.display()))?;
    sync_parent(path)
}

fn sync_file(path: &Path) -> Result<(), String> {
    fs::File::open(path)
        .map_err(|error| format!("open {} to sync it: {error}", path.display()))?
        .sync_all()
        .map_err(|error| format!("fsync {}: {error}", path.display()))?;
    sync_parent(path)
}
