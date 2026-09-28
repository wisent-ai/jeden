//! Putting something into memory so the record of what was believed, and
//! when, survives being corrected.

use super::super::*;
use super::{logical_key, row_record, MemoryStore, RECORD_COLUMNS};
use crate::fleet::{run_db, sql};
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
        let logical_key = logical_key.trim().to_owned();
        if logical_key.is_empty() {
            return Err("memory logical key is empty".into());
        }
        let now = now_ms();
        let valid_from = valid_from.unwrap_or(now);
        let (kind, scope) = (kind.to_owned(), scope.clone());
        let tags = serde_json::to_string(tags).map_err(|e| e.to_string())?;
        let source = serde_json::to_string(source).map_err(|e| e.to_string())?;
        let confidence = confidence.clamp(0.0, 1.0);
        run_db(move |client| {
            let mut tx = client.transaction().map_err(sql)?;
            let prior = tx
                .query_opt(
                    &*format!(
                        "SELECT {RECORD_COLUMNS} FROM memories WHERE scope_kind=$1 AND scope_id=$2
                         AND logical_key=$3 AND valid_to IS NULL ORDER BY revision DESC LIMIT 1 FOR UPDATE"
                    ),
                    &[&scope.kind, &scope.id, &logical_key],
                )
                .map_err(sql)?
                .map(|row| row_record(&row));
            if let Some(existing) = prior.as_ref().filter(|record| {
                record.text == text && !record.tombstone && record.status == "active"
            }) {
                tx.commit().map_err(sql)?;
                return Ok(existing.clone());
            }
            let revision = prior
                .as_ref()
                .map(|record| record.revision + 1)
                .unwrap_or(1);
            let supersedes = prior.as_ref().map(|record| record.id.clone());
            if let Some(prior) = &prior {
                tx.execute(
                    "UPDATE memories SET valid_to=$2,status='superseded',updated_at=$2 WHERE id=$1",
                    &[&prior.id, &valid_from],
                )
                .map_err(sql)?;
            }
            let id = stable_id("mem");
            tx.execute(
                "INSERT INTO memories(id,kind,scope_kind,scope_id,text,tags_json,source_json,confidence,status,
                 created_at,updated_at,logical_key,revision,valid_from,valid_to,supersedes,tombstone)
                 VALUES($1,$2,$3,$4,$5,$6,$7,$8,'active',$9,$9,$10,$11,$12,NULL,$13,FALSE)",
                &[&id, &kind, &scope.kind, &scope.id, &text, &tags, &source, &confidence, &now,
                  &logical_key, &revision, &valid_from, &supersedes],
            )
            .map_err(sql)?;
            tx.execute(
                "INSERT INTO memory_outbox(id,dedupe_key,event_kind,payload_json,available_at,created_at)
                 VALUES($1,$2,'memory-upserted',$3,$4,$4) ON CONFLICT DO NOTHING",
                &[&stable_id("evt"), &format!("memory-upserted:{id}"),
                  &json!({"memoryId": id}).to_string(), &now],
            )
            .map_err(sql)?;
            let record = tx
                .query_one(
                    &*format!("SELECT {RECORD_COLUMNS} FROM memories WHERE id=$1"),
                    &[&id],
                )
                .map_err(sql)?;
            let record = row_record(&record);
            tx.commit().map_err(sql)?;
            Ok(record)
        })
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
        let (scope, logical_key) = (scope.clone(), logical_key.to_owned());
        let source = serde_json::to_string(source).map_err(|e| e.to_string())?;
        run_db(move |client| {
            let mut tx = client.transaction().map_err(sql)?;
            let prior = tx
                .query_opt(
                    &*format!(
                        "SELECT {RECORD_COLUMNS} FROM memories WHERE scope_kind=$1 AND scope_id=$2
                         AND logical_key=$3 AND valid_to IS NULL ORDER BY revision DESC LIMIT 1 FOR UPDATE"
                    ),
                    &[&scope.kind, &scope.id, &logical_key],
                )
                .map_err(sql)?
                .map(|row| row_record(&row))
                .ok_or("active logical memory not found")?;
            tx.execute(
                "UPDATE memories SET valid_to=$2,status='superseded',updated_at=$2 WHERE id=$1",
                &[&prior.id, &at],
            )
            .map_err(sql)?;
            let id = stable_id("mem");
            tx.execute(
                "INSERT INTO memories(id,kind,scope_kind,scope_id,text,tags_json,source_json,confidence,status,
                 created_at,updated_at,logical_key,revision,valid_from,supersedes,tombstone)
                 VALUES($1,$2,$3,$4,'','[]',$5,1.0,'active',$6,$6,$7,$8,$9,$10,TRUE)",
                &[&id, &prior.kind, &scope.kind, &scope.id, &source, &now, &logical_key,
                  &(prior.revision + 1), &at, &prior.id],
            )
            .map_err(sql)?;
            let record = tx
                .query_one(
                    &*format!("SELECT {RECORD_COLUMNS} FROM memories WHERE id=$1"),
                    &[&id],
                )
                .map_err(sql)?;
            let record = row_record(&record);
            tx.commit().map_err(sql)?;
            Ok(record)
        })
    }

    pub fn forget_scope(&self, scope: &MemoryScope) -> Result<usize, String> {
        let scope = scope.clone();
        run_db(move |client| {
            Ok(client
                .execute(
                    "UPDATE memories SET status='forgotten',valid_to=COALESCE(valid_to,$3),updated_at=$3
                     WHERE scope_kind=$1 AND scope_id=$2 AND status='active'",
                    &[&scope.kind, &scope.id, &now_ms()],
                )
                .map_err(sql)? as usize)
        })
    }
    pub fn clear(&self) -> Result<usize, String> {
        run_db(|client| {
            let mut tx = client.transaction().map_err(sql)?;
            // Revisions point at the rows they supersede; unlink them first.
            tx.execute("UPDATE memories SET supersedes=NULL", &[])
                .map_err(sql)?;
            let removed = tx.execute("DELETE FROM memories", &[]).map_err(sql)?;
            tx.commit().map_err(sql)?;
            Ok(removed as usize)
        })
    }
}
