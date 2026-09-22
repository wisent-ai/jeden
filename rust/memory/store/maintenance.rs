//! Checking that the memory database is sound, and rebuilding what can be
//! rebuilt from what is already stored.
//!
//! Split out of `memory/store.rs`, which had grown past the module line cap.

use super::super::*;
use super::MemoryStore;
use rusqlite::Connection;
use serde_json::{json, Value};

impl MemoryStore {
    pub fn rebuild_fts(&self) -> Result<Value, String> {
        let mut conn = self.connect()?;
        let memory_rows: i64 = conn
            .query_row("SELECT count(*) FROM memories", [], |row| row.get(0))
            .map_err(|e| e.to_string())?;
        let tx = conn
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(|e| e.to_string())?;
        tx.execute(
            "INSERT INTO memories_fts(memories_fts) VALUES('delete-all')",
            [],
        )
        .map_err(|e| format!("FTS5 index reset failed: {e}"))?;
        tx.execute(
            "INSERT INTO memories_fts(rowid,text,tags,kind) SELECT rowid,text,tags_json,kind FROM memories",
            [],
        )
        .map_err(|e| format!("FTS5 repopulation failed: {e}"))?;
        tx.execute(
            "INSERT INTO memories_fts(memories_fts) VALUES('optimize')",
            [],
        )
        .map_err(|e| format!("FTS5 optimize failed: {e}"))?;
        fts_integrity_check(&tx)?;
        tx.commit().map_err(|e| e.to_string())?;
        let (busy, wal_frames, checkpointed_frames): (i64, i64, i64) = conn
            .query_row("PRAGMA wal_checkpoint(PASSIVE)", [], |row| {
                Ok((row.get(0)?, row.get(1)?, row.get(2)?))
            })
            .map_err(|e| format!("WAL checkpoint status failed: {e}"))?;
        Ok(json!({
            "backend": "sqlite-wal-fts5",
            "operation": "fts-rebuild-optimize",
            "integrity": "ok",
            "memoryRows": memory_rows,
            "walCheckpoint": {
                "busy": busy != 0,
                "logFrames": wal_frames,
                "checkpointedFrames": checkpointed_frames,
            },
        }))
    }

    pub fn health(&self) -> Result<Value, String> {
        let conn = self.connect()?;
        let memories:i64=conn.query_row("SELECT count(*) FROM memories WHERE status='active' AND tombstone=0 AND valid_to IS NULL",[],|r|r.get(0)).map_err(|e|e.to_string())?;
        let sqlite_integrity: String = conn
            .query_row("PRAGMA quick_check(1)", [], |row| row.get(0))
            .map_err(|e| format!("SQLite quick_check failed: {e}"))?;
        let fts_integrity = match fts_integrity_check(&conn) {
            Ok(()) => "ok".to_string(),
            Err(error) => error,
        };
        let queue = self.queue_status(20)?;
        let embedding = embeddings::health(&conn, None)?;
        let healthy = sqlite_integrity == "ok" && fts_integrity == "ok" && queue.failed == 0;
        Ok(json!({
            "service": "memory",
            "healthy": healthy,
            "backend": "sqlite-wal-fts5",
            "retrievalMode": embedding.mode,
            "embeddingAvailable": embedding.available,
            "schemaVersion": schema::SCHEMA_VERSION,
            "path": self.path,
            "activeMemories": memories,
            "pendingJobs": queue.pending,
            "failedJobs": queue.failed,
            "queue": queue,
            "integrity": {
                "sqlite": sqlite_integrity,
                "fts5": fts_integrity,
            },
            "provenance": true,
            "bounded": {
                "memoryChars": MAX_MEMORY_CHARS,
                "contextChars": MAX_CONTEXT_CHARS,
                "attempts": MAX_ATTEMPTS,
            },
        }))
    }
}
}
fn fts_integrity_check(conn: &Connection) -> Result<(), String> {
    conn.execute(
        "INSERT INTO memories_fts(memories_fts) VALUES('integrity-check')",
        [],
    )
    .map(|_| ())
    .map_err(|e| format!("FTS5 integrity-check failed: {e}"))
}
