//! Getting memories back out: what is there, what matches, and what was
//! believed at a given moment.

use super::super::*;
use super::{load_record, row_record, MemoryStore, RECORD_COLUMNS};
use crate::fleet::{run_db, sql};
use serde_json::json;

impl MemoryStore {
    pub fn list(&self, limit: usize) -> Result<Vec<MemoryRecord>, String> {
        let limit = limit.min(500) as i64;
        run_db(move |client| {
            Ok(client
                .query(
                    &*format!("SELECT {RECORD_COLUMNS} FROM memories ORDER BY updated_at DESC,id LIMIT $1"),
                    &[&limit],
                )
                .map_err(sql)?
                .iter()
                .map(row_record)
                .collect())
        })
    }

    pub fn recall(
        &self,
        backend: &dyn SemanticBackend,
        scope: &MemoryScope,
        query: &str,
        limit: usize,
    ) -> Result<Vec<RecallHit>, String> {
        let ranked = backend.recall(scope, query, limit.min(100))?;
        let ids = ranked
            .iter()
            .map(|candidate| candidate.id.clone())
            .collect::<Vec<_>>();
        let (groups, records) = run_db(move |client| {
            let groups = conflict::conflict_groups(client, &ids)?;
            let mut records = Vec::new();
            for id in &ids {
                let record = load_record(client, id)?;
                let edges = match &record {
                    Some(record) => conflict::edges(client, &record.id)?,
                    None => Vec::new(),
                };
                records.push(record.map(|record| (record, edges)));
            }
            Ok((groups, records))
        })?;
        let mut hits = Vec::new();
        for (candidate, found) in ranked.into_iter().zip(records) {
            let Some((record, edges)) = found else {
                continue;
            };
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
        let (from_id, to_id, source) = (from_id.to_owned(), to_id.to_owned(), source.clone());
        run_db(move |client| conflict::add_edge(client, &from_id, &to_id, relation, &source))
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
        let winner_id = winner_id.to_owned();
        let loser_ids = loser_ids.to_vec();
        let payload =
            json!({"winnerId": winner_id, "loserIds": loser_ids, "source": source}).to_string();
        run_db(move |client| {
            let mut tx = client.transaction().map_err(sql)?;
            let winner_exists: bool = tx
                .query_one(
                    "SELECT EXISTS(SELECT 1 FROM memories WHERE id=$1 AND status='active' AND NOT tombstone)",
                    &[&winner_id],
                )
                .map_err(sql)?
                .get(0);
            if !winner_exists {
                return Err("conflict winner is not active".into());
            }
            let mut resolved = 0;
            for loser in &loser_ids {
                resolved += tx
                    .execute(
                        "UPDATE memories SET status='resolved',valid_to=COALESCE(valid_to,$2),updated_at=$2
                         WHERE id=$1 AND status='active'",
                        &[loser, &now],
                    )
                    .map_err(sql)? as usize;
            }
            tx.execute(
                "INSERT INTO memory_outbox(id,dedupe_key,event_kind,payload_json,available_at,created_at)
                 VALUES($1,$2,'conflict-resolved',$3,$4,$4) ON CONFLICT DO NOTHING",
                &[&stable_id("evt"), &format!("conflict-resolved:{winner_id}:{}", loser_ids.join(":")),
                  &payload, &now],
            )
            .map_err(sql)?;
            tx.commit().map_err(sql)?;
            Ok(resolved)
        })
    }
}
