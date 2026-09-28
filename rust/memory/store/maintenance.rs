//! Checking that the memory tables answer, and rebuilding the lexical index
//! from what is already stored.

use super::super::*;
use super::MemoryStore;
use crate::fleet::{run_db, sql};
use serde_json::{json, Value};

impl MemoryStore {
    /// Rebuilds the lexical index. The index covers a generated column, so
    /// it is never stale; rebuilding compacts it and proves it is readable.
    pub fn rebuild_fts(&self) -> Result<Value, String> {
        let memory_rows: i64 = run_db(|client| {
            client
                .batch_execute("REINDEX INDEX memories_search")
                .map_err(|e| format!("rebuilding memories_search failed: {e}"))?;
            Ok(client
                .query_one("SELECT count(*) FROM memories", &[])
                .map_err(sql)?
                .get(0))
        })?;
        Ok(json!({
            "backend": "fleet-postgres-fts",
            "operation": "reindex",
            "index": "memories_search",
            "memoryRows": memory_rows,
        }))
    }

    pub fn health(&self) -> Result<Value, String> {
        let memories: i64 = run_db(|client| {
            Ok(client
                .query_one(
                    "SELECT count(*) FROM memories WHERE status='active' AND NOT tombstone AND valid_to IS NULL",
                    &[],
                )
                .map_err(sql)?
                .get(0))
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
                "contextChars": MAX_CONTEXT_CHARS,
                "attempts": MAX_ATTEMPTS,
            },
        }))
    }
}
