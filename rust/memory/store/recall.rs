//! Getting memories back out: what is there, what matches, and what was
//! believed at a given moment.
//!
//! Split out of `memory/store.rs`, which had grown past the module line cap.

use super::super::*;
use super::{row_record, MemoryStore};

impl MemoryStore {
    pub fn list(&self, limit: usize) -> Result<Vec<MemoryRecord>, String> {
        let conn = self.connect()?;
        let mut stmt=conn.prepare("SELECT id,kind,scope_kind,scope_id,text,tags_json,source_json,confidence,status,created_at,updated_at,logical_key,revision,valid_from,valid_to,supersedes,tombstone FROM memories ORDER BY updated_at DESC,id LIMIT ?1").map_err(|e|e.to_string())?;
        let rows = stmt
            .query_map([limit.min(500) as i64], row_record)
            .map_err(|e| e.to_string())?
            .collect::<Result<Vec<_>, _>>()
            .map_err(|e| e.to_string())?;
        Ok(rows)
    }

    pub fn recall(
        &self,
        backend: &dyn SemanticBackend,
        scope: &MemoryScope,
        query: &str,
        limit: usize,
    ) -> Result<Vec<RecallHit>, String> {
        let conn = self.connect()?;
        let ranked = backend.recall(&conn, scope, query, limit.min(100))?;
        let ids = ranked
            .iter()
            .map(|candidate| candidate.id.clone())
            .collect::<Vec<_>>();
        let groups = conflict::conflict_groups(&conn, &ids)?;
        let mut hits = Vec::new();
        for candidate in ranked {
            if let Some(record) = load_record(&conn, &candidate.id)? {
                let edges = conflict::edges(&conn, &record.id)?;
                hits.push(RecallHit {
                    score: candidate.score,
                    components: candidate.components,
                    conflict_group: groups.get(&record.id).cloned(),
                    provenance: RecallProvenance {
                        backend: backend.name().into(),
                        query: query.into(),
                        source: record.source.clone(),
                        memory_id: record.id.clone(),
                        logical_key: record.logical_key.clone(),
                        revision: record.revision,
                        edges,
                    },
                    record,
                })
            }
        }
        Ok(hits)
    }

    pub fn recall_at(
        &self,
        scope: &MemoryScope,
        query: &str,
        limit: usize,
        as_of: i64,
    ) -> Result<Vec<RecallHit>, String> {
        let backend = HybridBackend {
            provider: None,
            as_of: Some(as_of),
            half_life_ms: 30.0 * 24.0 * 60.0 * 60.0 * 1_000.0,
        };
        self.recall(&backend, scope, query, limit)
    }
    pub fn add_edge(
        &self,
        from_id: &str,
        to_id: &str,
        relation: MemoryRelation,
        source: &MemorySource,
    ) -> Result<(), String> {
        conflict::add_edge(&self.connect()?, from_id, to_id, relation, source)
    }
    pub fn resolve_conflict(
        &self,
        winner_id: &str,
        loser_ids: &[String],
        source: &MemorySource,
    ) -> Result<usize, String> {
        if loser_ids.iter().any(|id| id == winner_id) {
            return Err("conflict winner cannot also be a loser".into());
        }
        let now = now_ms();
        let mut conn = self.connect()?;
        let tx = conn
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(|e| e.to_string())?;
        let winner_exists:bool=tx.query_row("SELECT EXISTS(SELECT 1 FROM memories WHERE id=?1 AND status='active' AND tombstone=0)",[winner_id],|row|row.get(0)).map_err(|e|e.to_string())?;
        if !winner_exists {
            return Err("conflict winner is not active".into());
        }
        let mut resolved = 0;
        for loser in loser_ids {
            resolved+=tx.execute("UPDATE memories SET status='resolved',valid_to=COALESCE(valid_to,?2),updated_at=?2 WHERE id=?1 AND status='active'",params![loser,now]).map_err(|e|e.to_string())?;
        }
        tx.execute("INSERT OR IGNORE INTO memory_outbox(id,dedupe_key,event_kind,payload_json,available_at,created_at) VALUES(?1,?2,'conflict-resolved',?3,?4,?4)",params![stable_id("evt"),format!("conflict-resolved:{winner_id}:{}",loser_ids.join(":")),json!({"winnerId":winner_id,"loserIds":loser_ids,"source":source}).to_string(),now]).map_err(|e|e.to_string())?;
        tx.commit().map_err(|e| e.to_string())?;
        Ok(resolved)
    }

}
