mod journal;
mod plan;

pub(crate) use plan::{CompatibilityWindow, MigrationPlan, MigrationStep};

use journal::{journal_path, sync_parent, write_durable, JournalStage, MigrationJournal};
use rusqlite::{Connection, TransactionBehavior};
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

fn version(value: &Value) -> Result<u32, String> {
    match value.get("schemaVersion") {
        None => Ok(0),
        Some(value) => value
            .as_u64()
            .and_then(|v| u32::try_from(v).ok())
            .ok_or_else(|| "schemaVersion must be a non-negative 32-bit integer".into()),
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
    if !journal_file.exists() {
        return Ok(None);
    }
    let state = journal::read(&journal_file)?;
    if state.target != path || state.store != plan.store || state.to != plan.to {
        return Err(format!(
            "migration journal {} does not match requested store",
            journal_file.display()
        ));
    }
    match state.stage {
        JournalStage::Prepared | JournalStage::BackupDurable => {
            if state.staged.exists() {
                fs::remove_file(&state.staged).map_err(|error| error.to_string())?;
            }
            if !path.exists() {
                let backup = state
                    .backup
                    .as_ref()
                    .filter(|backup| backup.exists())
                    .ok_or_else(|| {
                        format!(
                            "migration recovery has neither old store nor backup for {}",
                            path.display()
                        )
                    })?;
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
            if state.staged.exists() {
                fs::remove_file(&state.staged).map_err(|error| error.to_string())?;
            }
        }
    }
    fs::remove_file(&journal_file).map_err(|error| error.to_string())?;
    sync_parent(path)?;
    let value: Value = serde_json::from_slice(&fs::read(path).map_err(|error| error.to_string())?)
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
    let mut value: Value = serde_json::from_slice(&original)
        .or_else(|_| serde_yaml::from_slice(&original))
        .map_err(|error| format!("corrupt {} store {}: {error}", plan.store, path.display()))?;
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

    let staged = sibling(path, "migration-stage");
    let backup = path.with_extension(format!("backup-v{from}"));
    write_synced(
        &staged,
        &serde_json::to_vec_pretty(&value).map_err(|error| error.to_string())?,
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
    fs::remove_file(&journal_file).map_err(|error| error.to_string())?;
    sync_parent(path)?;
    Ok(MigrationOutcome::Migrated {
        from,
        to: plan.to,
        backup: Some(backup),
    })
}

pub(crate) fn migrate_sqlite<F>(
    path: &Path,
    plan: &MigrationPlan,
    mut apply: F,
) -> Result<MigrationOutcome, String>
where
    F: FnMut(&rusqlite::Transaction<'_>, u32, u32) -> Result<(), String>,
{
    plan.validate()?;
    let journal_file = journal_path(path);
    if journal_file.exists() {
        let state = journal::read(&journal_file)?;
        if state.target != path || state.store != plan.store {
            return Err(format!(
                "migration journal {} does not match requested store",
                journal_file.display()
            ));
        }
        let recovery = Connection::open(path).map_err(|error| error.to_string())?;
        let integrity: String = recovery
            .query_row("PRAGMA integrity_check", [], |row| row.get(0))
            .map_err(|error| error.to_string())?;
        let found: u32 = recovery
            .query_row("PRAGMA user_version", [], |row| row.get(0))
            .map_err(|error| error.to_string())?;
        drop(recovery);
        if integrity == "ok" && (found == state.from || found == state.to) {
            fs::remove_file(&journal_file).map_err(|error| error.to_string())?;
            sync_parent(path)?;
            return Ok(MigrationOutcome::Recovered { version: found });
        }
        let backup = state
            .backup
            .as_ref()
            .filter(|candidate| candidate.exists())
            .ok_or_else(|| format!("cannot recover {} without a durable backup", path.display()))?;
        fs::copy(backup, path).map_err(|error| format!("restore {}: {error}", path.display()))?;
        sync_file(path)?;
        fs::remove_file(&journal_file).map_err(|error| error.to_string())?;
        return Ok(MigrationOutcome::Recovered {
            version: state.from,
        });
    }
    let mut conn =
        Connection::open(path).map_err(|error| format!("open {}: {error}", path.display()))?;
    let integrity: String = conn
        .query_row("PRAGMA integrity_check", [], |row| row.get(0))
        .map_err(|error| error.to_string())?;
    if integrity != "ok" {
        return Err(format!(
            "corrupt {} store {}: {integrity}",
            plan.store,
            path.display()
        ));
    }
    let mut from: u32 = conn
        .query_row("PRAGMA user_version", [], |row| row.get(0))
        .map_err(|error| error.to_string())?;
    if from == 0 {
        from = conn
            .query_row(
                "SELECT CAST(value AS INTEGER) FROM metadata WHERE key='schema_version'",
                [],
                |row| row.get(0),
            )
            .unwrap_or(0);
    }
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
    conn.execute_batch("PRAGMA wal_checkpoint(TRUNCATE)")
        .map_err(|error| error.to_string())?;
    let backup = path.with_extension(format!("backup-v{from}"));
    let staged = sibling(path, "sqlite-transaction");
    let mut state = MigrationJournal {
        store: plan.store.into(),
        from,
        to: plan.to,
        target: path.into(),
        staged,
        backup: Some(backup.clone()),
        stage: JournalStage::Prepared,
    };
    write_durable(&journal_file, &state)?;
    fs::copy(path, &backup).map_err(|error| format!("backup {}: {error}", path.display()))?;
    sync_file(&backup)?;
    state.stage = JournalStage::BackupDurable;
    write_durable(&journal_file, &state)?;
    let tx = conn
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .map_err(|error| error.to_string())?;
    let mut current = from;
    for step in plan.steps.iter().filter(|step| step.from >= from) {
        apply(&tx, step.from, step.to)?;
        current = step.to;
        tx.pragma_update(None, "user_version", current)
            .map_err(|error| error.to_string())?;
        let has_metadata: bool = tx
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type='table' AND name='metadata')",
                [],
                |row| row.get(0),
            )
            .map_err(|error| error.to_string())?;
        if has_metadata {
            tx.execute("INSERT INTO metadata(key,value) VALUES('schema_version',?1) ON CONFLICT(key) DO UPDATE SET value=excluded.value", [current.to_string()]).map_err(|error| error.to_string())?;
        }
    }
    if current != plan.to {
        return Err(format!(
            "{} migration stopped at {}, expected {}",
            plan.store, current, plan.to
        ));
    }
    tx.commit().map_err(|error| error.to_string())?;
    conn.execute_batch("PRAGMA wal_checkpoint(FULL)")
        .map_err(|error| error.to_string())?;
    sync_file(path)?;
    state.stage = JournalStage::Committed;
    write_durable(&journal_file, &state)?;
    fs::remove_file(&journal_file).map_err(|error| error.to_string())?;
    sync_parent(path)?;
    Ok(MigrationOutcome::Migrated {
        from,
        to: plan.to,
        backup: Some(backup),
    })
}

pub(crate) fn write_json_atomic(path: &Path, value: &Value) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|error| error.to_string())?;
    }
    let staged = sibling(path, "write-stage");
    write_synced(
        &staged,
        &serde_json::to_vec_pretty(value).map_err(|error| error.to_string())?,
    )?;
    fs::rename(&staged, path)
        .map_err(|error| format!("atomic commit {}: {error}", path.display()))?;
    sync_parent(path)
}

fn sibling(path: &Path, suffix: &str) -> PathBuf {
    let name = path
        .file_name()
        .and_then(|value| value.to_str())
        .unwrap_or("store");
    path.with_file_name(format!(".{name}.{suffix}-{}", std::process::id()))
}

fn write_synced(path: &Path, bytes: &[u8]) -> Result<(), String> {
    let mut file = OpenOptions::new()
        .create(true)
        .truncate(true)
        .write(true)
        .open(path)
        .map_err(|error| format!("create {}: {error}", path.display()))?;
    file.write_all(bytes).map_err(|error| error.to_string())?;
    file.write_all(b"\n").map_err(|error| error.to_string())?;
    file.sync_all()
        .map_err(|error| format!("fsync {}: {error}", path.display()))?;
    sync_parent(path)
}

fn sync_file(path: &Path) -> Result<(), String> {
    fs::File::open(path)
        .map_err(|error| error.to_string())?
        .sync_all()
        .map_err(|error| error.to_string())?;
    sync_parent(path)
}

fn envelope_step(value: &mut Value) -> Result<(), String> {
    object_preflight(value)
}
static ENVELOPE_STEPS: [MigrationStep; 2] = [
    MigrationStep {
        name: "version-envelope",
        from: 0,
        to: 1,
        apply: envelope_step,
    },
    MigrationStep {
        name: "compatibility-window-v2",
        from: 1,
        to: 2,
        apply: envelope_step,
    },
];

pub(crate) fn builtin_document_plan(store: &'static str) -> Result<MigrationPlan, String> {
    if !matches!(
        store,
        "session-ledger" | "task-store" | "plugin-lock" | "quality"
    ) {
        return Err(format!("unknown built-in migration store: {store}"));
    }
    Ok(MigrationPlan {
        store,
        from: 0,
        to: 2,
        reversible: true,
        preflight: object_preflight,
        steps: &ENVELOPE_STEPS,
        compatibility_window: CompatibilityWindow {
            oldest_readable: 0,
            newest_readable: 2,
            rollback_floor: 1,
        },
    })
}

pub(crate) fn object_preflight(value: &Value) -> Result<(), String> {
    if value.is_object() {
        Ok(())
    } else {
        Err("store root must be an object".into())
    }
}
