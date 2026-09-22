//! Putting something into memory so the record of what was believed, and
//! when, survives being corrected.
//!
//! Split out of `memory/store.rs`, which had grown past the module line cap.

use super::super::*;
use super::{logical_key, MemoryStore};
use rusqlite::params;
use crate::memory::store::row_record;
use rusqlite::{OptionalExtension, TransactionBehavior};
use serde_json::json;

impl MemoryStore {
    pub fn remember(
        &self,
        kind: &str,
        scope: &MemoryScope,
        text: &str,
        tags: &[String],
        source: &MemorySource,
        confidence: f64,
    ) -> Result<MemoryRecord, String> {
        let redacted = bounded_redacted(text, MAX_MEMORY_CHARS);
        let logical_key = logical_key(kind, scope, &redacted);
        self.remember_with_key(
            kind,
            scope,
            &logical_key,
            &redacted,
            tags,
            source,
            confidence,
            None,
        )
    }

    // These are the caller-supplied columns of the row being written; the only
    // struct with this shape is `MemoryRecord`, which also carries the ids,
    // revision, and timestamps this call is the one to assign.
    #[allow(clippy::too_many_arguments)]
    pub fn remember_with_key(
        &self,
        kind: &str,
        scope: &MemoryScope,
        logical_key: &str,
        text: &str,
        tags: &[String],
        source: &MemorySource,
        confidence: f64,
        valid_from: Option<i64>,
    ) -> Result<MemoryRecord, String> {
        let text = bounded_redacted(text, MAX_MEMORY_CHARS);
        if text.is_empty() {
            return Err("memory text is empty after redaction".into());
        }
        let logical_key = logical_key.trim();
        if logical_key.is_empty() {
            return Err("memory logical key is empty".into());
        }
        let now = now_ms();
        let valid_from = valid_from.unwrap_or(now);
        let mut conn = self.connect()?;
        let tx = conn
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(|e| e.to_string())?;
        let prior:Option<MemoryRecord>=tx.query_row(
            "SELECT id,kind,scope_kind,scope_id,text,tags_json,source_json,confidence,status,created_at,updated_at,logical_key,revision,valid_from,valid_to,supersedes,tombstone FROM memories WHERE scope_kind=?1 AND scope_id=?2 AND logical_key=?3 AND valid_to IS NULL ORDER BY revision DESC LIMIT 1",
            params![scope.kind,scope.id,logical_key],row_record).optional().map_err(|e|e.to_string())?;
        if let Some(existing) = prior
            .as_ref()
            .filter(|record| record.text == text && !record.tombstone && record.status == "active")
        {
            tx.commit().map_err(|e| e.to_string())?;
            return Ok(existing.clone());
        }
        let revision = prior
            .as_ref()
            .map(|record| record.revision + 1)
            .unwrap_or(1);
        let supersedes = prior.as_ref().map(|record| record.id.clone());
        if let Some(prior) = &prior {
            tx.execute(
                "UPDATE memories SET valid_to=?2,status='superseded',updated_at=?2 WHERE id=?1",
                params![prior.id, valid_from],
            )
            .map_err(|e| e.to_string())?;
        }
        let id = stable_id("mem");
        tx.execute("INSERT INTO memories(id,kind,scope_kind,scope_id,text,tags_json,source_json,confidence,status,created_at,updated_at,logical_key,revision,valid_from,valid_to,supersedes,tombstone) VALUES(?1,?2,?3,?4,?5,?6,?7,?8,'active',?9,?9,?10,?11,?12,NULL,?13,0)",params![id,kind,scope.kind,scope.id,text,serde_json::to_string(tags).map_err(|e|e.to_string())?,serde_json::to_string(source).map_err(|e|e.to_string())?,confidence.clamp(0.0,1.0),now,logical_key,revision,valid_from,supersedes]).map_err(|e|e.to_string())?;
        tx.execute("INSERT OR IGNORE INTO memory_outbox(id,dedupe_key,event_kind,payload_json,available_at,created_at) VALUES(?1,?2,'memory-upserted',?3,?4,?4)",params![stable_id("evt"),format!("memory-upserted:{id}"),json!({"memoryId":id}).to_string(),now]).map_err(|e|e.to_string())?;
        let record=tx.query_row("SELECT id,kind,scope_kind,scope_id,text,tags_json,source_json,confidence,status,created_at,updated_at,logical_key,revision,valid_from,valid_to,supersedes,tombstone FROM memories WHERE id=?1",[&id],row_record).map_err(|e|e.to_string())?;
        tx.commit().map_err(|e| e.to_string())?;
        Ok(record)
    }

    pub fn tombstone(
        &self,
        scope: &MemoryScope,
        logical_key: &str,
        source: &MemorySource,
        valid_from: Option<i64>,
    ) -> Result<MemoryRecord, String> {
        let now = now_ms();
        let at = valid_from.unwrap_or(now);
        let mut conn = self.connect()?;
        let tx = conn
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(|e| e.to_string())?;
        let prior:MemoryRecord=tx.query_row("SELECT id,kind,scope_kind,scope_id,text,tags_json,source_json,confidence,status,created_at,updated_at,logical_key,revision,valid_from,valid_to,supersedes,tombstone FROM memories WHERE scope_kind=?1 AND scope_id=?2 AND logical_key=?3 AND valid_to IS NULL ORDER BY revision DESC LIMIT 1",params![scope.kind,scope.id,logical_key],row_record).map_err(|_|"active logical memory not found".to_string())?;
        tx.execute(
            "UPDATE memories SET valid_to=?2,status='superseded',updated_at=?2 WHERE id=?1",
            params![prior.id, at],
        )
        .map_err(|e| e.to_string())?;
        let id = stable_id("mem");
        tx.execute("INSERT INTO memories(id,kind,scope_kind,scope_id,text,tags_json,source_json,confidence,status,created_at,updated_at,logical_key,revision,valid_from,supersedes,tombstone) VALUES(?1,?2,?3,?4,'','[]',?5,1.0,'active',?6,?6,?7,?8,?9,?10,1)",params![id,prior.kind,scope.kind,scope.id,serde_json::to_string(source).map_err(|e|e.to_string())?,now,logical_key,prior.revision+1,at,prior.id]).map_err(|e|e.to_string())?;
        let record=tx.query_row("SELECT id,kind,scope_kind,scope_id,text,tags_json,source_json,confidence,status,created_at,updated_at,logical_key,revision,valid_from,valid_to,supersedes,tombstone FROM memories WHERE id=?1",[&id],row_record).map_err(|e|e.to_string())?;
        tx.commit().map_err(|e| e.to_string())?;
        Ok(record)
    }

    pub fn forget_scope(&self, scope: &MemoryScope) -> Result<usize, String> {
        let conn = self.connect()?;
        conn.execute("UPDATE memories SET status='forgotten',valid_to=COALESCE(valid_to,?3),updated_at=?3 WHERE scope_kind=?1 AND scope_id=?2 AND status='active'",params![scope.kind,scope.id,now_ms()]).map_err(|e|e.to_string())
    }
    pub fn clear(&self) -> Result<usize, String> {
        self.connect()?
            .execute("DELETE FROM memories", [])
            .map_err(|e| e.to_string())
    }
}
