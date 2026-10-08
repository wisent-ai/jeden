//! Checking that the memory tables answer, and rebuilding the lexical index
//! from what is already stored.

use super::super::*;
use super::entities::memory;
use super::MemoryStore;
use crate::fleet::{run_db, sql};
use sea_orm::{ColumnTrait, ConnectionTrait, EntityTrait, PaginatorTrait, QueryFilter};
use serde_json::{json, Value};

impl MemoryStore {
    /// Rebuilds the lexical index. The index covers a generated column, so
    /// it is never stale; rebuilding compacts it and proves it is readable.
    pub fn rebuild_fts(&self) -> Result<Value, String> {
        let memory_rows = run_db(|db| async move {
            db.execute_unprepared("REINDEX INDEX memories_search")
                .await
                .map_err(|e| format!("rebuilding memories_search failed: {e}"))?;
            memory::Entity::find().count(&db).await.map_err(sql)
        })?;
        Ok(json!({
            "backend": "fleet-postgres-fts",
            "operation": "reindex",
            "index": "memories_search",
            "memoryRows": memory_rows,
        }))
    }

    pub fn health(&self) -> Result<Value, String> {
        let memories = run_db(|db| async move {
            memory::Entity::find()
                .filter(memory::Column::Status.eq("active"))
                .filter(memory::Column::Tombstone.eq(false))
                .filter(memory::Column::ValidTo.is_null())
                .count(&db)
                .await
                .map_err(sql)
        })?;
        let queue = self.queue_status(20)?;
        let embedding = embeddings::health(None)?;
        let healthy = queue.failed == 0;
        Ok(json!({
            "service": "memory",
            "healthy": healthy,
            "backend": "fleet-postgres-fts",
            "retrievalMode": embedding.mode,
            "embeddingAvailable": embedding.available,
            "location": self.location(),
            "activeMemories": memories,
            "pendingJobs": queue.pending,
            "failedJobs": queue.failed,
            "queue": queue,
            "provenance": true,
            "bounded": {
                "memoryChars": MAX_MEMORY_CHARS,
                "attempts": MAX_ATTEMPTS,
            },
        }))
    }
}
