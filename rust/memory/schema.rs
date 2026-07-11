use rusqlite::{Connection, Transaction};
use std::path::Path;

pub(super) const SCHEMA_VERSION: i64 = 3;

fn marker(value: &mut serde_json::Value) -> Result<(), String> {
    crate::cli::config::migrations::object_preflight(value)
}

static STEPS: [crate::cli::config::migrations::MigrationStep; 3] = [
    crate::cli::config::migrations::MigrationStep {
        name: "legacy-schema-baseline",
        from: 0,
        to: 1,
        apply: marker,
    },
    crate::cli::config::migrations::MigrationStep {
        name: "migration-history",
        from: 1,
        to: 2,
        apply: marker,
    },
    crate::cli::config::migrations::MigrationStep {
        name: "revision-aware-semantic-memory",
        from: 2,
        to: 3,
        apply: marker,
    },
];

fn plan() -> crate::cli::config::migrations::MigrationPlan {
    crate::cli::config::migrations::MigrationPlan {
        store: "memory",
        from: 0,
        to: SCHEMA_VERSION as u32,
        reversible: true,
        preflight: crate::cli::config::migrations::object_preflight,
        steps: &STEPS,
        compatibility_window: crate::cli::config::migrations::CompatibilityWindow {
            oldest_readable: 0,
            newest_readable: SCHEMA_VERSION as u32,
            rollback_floor: 1,
        },
    }
}

pub(super) fn initialize(conn: &Connection) -> Result<(), String> {
    conn.execute_batch(
        "PRAGMA journal_mode=WAL; PRAGMA synchronous=FULL; PRAGMA foreign_keys=ON;
         CREATE TABLE IF NOT EXISTS metadata(key TEXT PRIMARY KEY, value TEXT NOT NULL);
         CREATE TABLE IF NOT EXISTS memories(
            id TEXT PRIMARY KEY, kind TEXT NOT NULL, scope_kind TEXT NOT NULL, scope_id TEXT NOT NULL,
            text TEXT NOT NULL, tags_json TEXT NOT NULL, source_json TEXT NOT NULL,
            confidence REAL NOT NULL CHECK(confidence BETWEEN 0 AND 1), status TEXT NOT NULL,
            created_at INTEGER NOT NULL, updated_at INTEGER NOT NULL);
         CREATE INDEX IF NOT EXISTS memories_scope ON memories(scope_kind,scope_id,status,updated_at DESC);
         CREATE VIRTUAL TABLE IF NOT EXISTS memories_fts USING fts5(text,tags,kind,content='memories',content_rowid='rowid');
         CREATE TRIGGER IF NOT EXISTS memories_ai AFTER INSERT ON memories BEGIN
            INSERT INTO memories_fts(rowid,text,tags,kind) VALUES(new.rowid,new.text,new.tags_json,new.kind); END;
         CREATE TRIGGER IF NOT EXISTS memories_ad AFTER DELETE ON memories BEGIN
            INSERT INTO memories_fts(memories_fts,rowid,text,tags,kind) VALUES('delete',old.rowid,old.text,old.tags_json,old.kind); END;
         CREATE TRIGGER IF NOT EXISTS memories_au AFTER UPDATE ON memories BEGIN
            INSERT INTO memories_fts(memories_fts,rowid,text,tags,kind) VALUES('delete',old.rowid,old.text,old.tags_json,old.kind);
            INSERT INTO memories_fts(rowid,text,tags,kind) VALUES(new.rowid,new.text,new.tags_json,new.kind); END;
         CREATE TABLE IF NOT EXISTS jobs(id TEXT PRIMARY KEY,kind TEXT NOT NULL,payload_json TEXT NOT NULL,state TEXT NOT NULL,attempts INTEGER NOT NULL DEFAULT 0,available_at INTEGER NOT NULL,lease_owner TEXT,lease_until INTEGER,heartbeat_at INTEGER,last_error TEXT,created_at INTEGER NOT NULL,updated_at INTEGER NOT NULL);
         CREATE INDEX IF NOT EXISTS jobs_claim ON jobs(state,available_at,lease_until);
         CREATE TABLE IF NOT EXISTS scope_locks(scope_kind TEXT NOT NULL,scope_id TEXT NOT NULL,owner TEXT NOT NULL,expires_at INTEGER NOT NULL,PRIMARY KEY(scope_kind,scope_id));
         CREATE TABLE IF NOT EXISTS workflows(fingerprint TEXT PRIMARY KEY,description TEXT NOT NULL,sessions_json TEXT NOT NULL,occurrences INTEGER NOT NULL,verified INTEGER NOT NULL DEFAULT 0,updated_at INTEGER NOT NULL);
         CREATE TABLE IF NOT EXISTS managed_skills(id TEXT PRIMARY KEY,workflow_fingerprint TEXT NOT NULL UNIQUE REFERENCES workflows(fingerprint),body TEXT NOT NULL,created_at INTEGER NOT NULL);"
    ).map_err(|e| e.to_string())
}

pub(super) fn migrate(
    path: &Path,
) -> Result<crate::cli::config::migrations::MigrationOutcome, String> {
    crate::cli::config::migrations::migrate_sqlite(path, &plan(), |tx, _, to| {
        match to {
            2 => tx.execute_batch(
                "CREATE TABLE IF NOT EXISTS migration_history(version INTEGER PRIMARY KEY,applied_at INTEGER NOT NULL);
                 INSERT OR IGNORE INTO migration_history(version,applied_at) VALUES(2,unixepoch('now'));"
            ).map_err(|e| e.to_string())?,
            3 => migrate_v3(tx)?,
            _ => {}
        }
        Ok(())
    })
}

fn migrate_v3(tx: &Transaction<'_>) -> Result<(), String> {
    let columns = tx
        .prepare("PRAGMA table_info(memories)")
        .and_then(|mut stmt| {
            stmt.query_map([], |row| row.get::<_, String>(1))?
                .collect::<Result<Vec<_>, _>>()
        })
        .map_err(|e| e.to_string())?;
    for (name, sql) in [
        (
            "logical_key",
            "ALTER TABLE memories ADD COLUMN logical_key TEXT",
        ),
        (
            "revision",
            "ALTER TABLE memories ADD COLUMN revision INTEGER NOT NULL DEFAULT 1",
        ),
        (
            "valid_from",
            "ALTER TABLE memories ADD COLUMN valid_from INTEGER",
        ),
        (
            "valid_to",
            "ALTER TABLE memories ADD COLUMN valid_to INTEGER",
        ),
        (
            "supersedes",
            "ALTER TABLE memories ADD COLUMN supersedes TEXT REFERENCES memories(id)",
        ),
        (
            "tombstone",
            "ALTER TABLE memories ADD COLUMN tombstone INTEGER NOT NULL DEFAULT 0",
        ),
    ] {
        if !columns.iter().any(|column| column == name) {
            tx.execute_batch(sql).map_err(|e| e.to_string())?;
        }
    }
    let history_columns = tx
        .prepare("PRAGMA table_info(migration_history)")
        .and_then(|mut stmt| {
            stmt.query_map([], |row| row.get::<_, String>(1))?
                .collect::<Result<Vec<_>, _>>()
        })
        .map_err(|e| e.to_string())?;
    if !history_columns
        .iter()
        .any(|column| column == "migration_id")
    {
        tx.execute_batch("ALTER TABLE migration_history ADD COLUMN migration_id TEXT")
            .map_err(|e| e.to_string())?;
    }
    tx.execute_batch(
        "UPDATE memories SET logical_key=id WHERE logical_key IS NULL;
         UPDATE memories SET valid_from=created_at WHERE valid_from IS NULL;
         CREATE UNIQUE INDEX IF NOT EXISTS memories_revision ON memories(scope_kind,scope_id,logical_key,revision);
         CREATE INDEX IF NOT EXISTS memories_temporal ON memories(scope_kind,scope_id,valid_from,valid_to,tombstone,status);
         CREATE TABLE IF NOT EXISTS memory_edges(
            from_id TEXT NOT NULL REFERENCES memories(id) ON DELETE CASCADE,
            to_id TEXT NOT NULL REFERENCES memories(id) ON DELETE CASCADE,
            relation TEXT NOT NULL CHECK(relation IN ('supports','conflicts','duplicates')),
            created_at INTEGER NOT NULL, provenance_json TEXT NOT NULL DEFAULT '{}',
            PRIMARY KEY(from_id,to_id,relation));
         CREATE INDEX IF NOT EXISTS memory_edges_to ON memory_edges(to_id,relation);
         CREATE TABLE IF NOT EXISTS memory_embeddings(
            memory_id TEXT PRIMARY KEY REFERENCES memories(id) ON DELETE CASCADE,
            model TEXT NOT NULL,dimensions INTEGER NOT NULL,vector_json TEXT NOT NULL,
            content_hash TEXT NOT NULL,updated_at INTEGER NOT NULL);
         CREATE TABLE IF NOT EXISTS memory_outbox(
            id TEXT PRIMARY KEY,dedupe_key TEXT NOT NULL UNIQUE,event_kind TEXT NOT NULL,
            payload_json TEXT NOT NULL,state TEXT NOT NULL DEFAULT 'pending',attempts INTEGER NOT NULL DEFAULT 0,
            available_at INTEGER NOT NULL,lease_owner TEXT,lease_until INTEGER,last_error TEXT,
            created_at INTEGER NOT NULL,processed_at INTEGER);
         CREATE INDEX IF NOT EXISTS memory_outbox_claim ON memory_outbox(state,available_at,lease_until);
         CREATE TABLE IF NOT EXISTS memory_processed_events(
            consumer TEXT NOT NULL,event_id TEXT NOT NULL,processed_at INTEGER NOT NULL,
            PRIMARY KEY(consumer,event_id));
         UPDATE migration_history SET migration_id=CASE version
            WHEN 1 THEN 'memory-001-legacy-schema'
            WHEN 2 THEN 'memory-002-migration-history'
            WHEN 3 THEN 'memory-003-revision-semantic'
            ELSE 'memory-unknown-' || version END
         WHERE migration_id IS NULL;
         INSERT OR IGNORE INTO migration_history(version,applied_at,migration_id) VALUES
            (1,unixepoch('now'),'memory-001-legacy-schema'),
            (2,unixepoch('now'),'memory-002-migration-history'),
            (3,unixepoch('now'),'memory-003-revision-semantic');
         CREATE UNIQUE INDEX IF NOT EXISTS migration_history_id ON migration_history(migration_id);"
    ).map_err(|e| e.to_string())
}
