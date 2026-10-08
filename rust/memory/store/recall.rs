//! Getting memories back out: what is there, what matches, and what was
//! believed at a given moment.

use super::super::*;
use super::entities::{memory, outbox};
use super::{load_record, record, MemoryStore};
use crate::fleet::{run_db, sql};
use sea_orm::sea_query::{Expr, OnConflict};
use sea_orm::{
    ActiveValue::Set, ColumnTrait, EntityTrait, QueryFilter, QueryOrder, QuerySelect,
    TransactionTrait,
};
use serde_json::json;

impl MemoryStore {
    pub fn list(&self, limit: usize) -> Result<Vec<MemoryRecord>, String> {
        let limit = limit.min(500) as u64;
        run_db(move |db| async move {
            Ok(memory::Entity::find()
                .order_by_desc(memory::Column::UpdatedAt)
                .order_by_asc(memory::Column::Id)
                .limit(limit)
                .all(&db)
                .await
                .map_err(sql)?
                .into_iter()
                .map(record)
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
        let ranked = backend.recall(scope, query, limit)?;
        let ids = ranked
            .iter()
            .map(|candidate| candidate.id.clone())
            .collect::<Vec<_>>();
        let (groups, records) = run_db(move |db| async move {
            let groups = conflict::conflict_groups(&db, &ids).await?;
            let mut records = Vec::new();
            for id in &ids {
                let found = match load_record(&db, id).await? {
                    Some(record) => {
                        let edges = conflict::edges(&db, &record.id).await?;
                        Some((record, edges))
                    }
                    None => None,
                };
                records.push(found);
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
        run_db(move |db| async move {
            conflict::add_edge(&db, &from_id, &to_id, relation, &source).await
        })
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
        run_db(move |db| async move {
            let tx = db.begin().await.map_err(sql)?;
            let winner = memory::Entity::find_by_id(winner_id.clone())
                .filter(memory::Column::Status.eq("active"))
                .filter(memory::Column::Tombstone.eq(false))
                .one(&tx)
                .await
                .map_err(sql)?;
            if winner.is_none() {
                return Err("conflict winner is not active".into());
            }
            let resolved = memory::Entity::update_many()
                .col_expr(memory::Column::Status, Expr::value("resolved"))
                .col_expr(
                    memory::Column::ValidTo,
                    Expr::cust_with_values("COALESCE(valid_to, $1)", [now]),
                )
                .col_expr(memory::Column::UpdatedAt, Expr::value(now))
                .filter(memory::Column::Id.is_in(loser_ids.clone()))
                .filter(memory::Column::Status.eq("active"))
                .exec(&tx)
                .await
                .map_err(sql)?
                .rows_affected;
            outbox::Entity::insert(outbox::ActiveModel {
                id: Set(stable_id("evt")),
                dedupe_key: Set(format!(
                    "conflict-resolved:{winner_id}:{}",
                    loser_ids.join(":")
                )),
                event_kind: Set("conflict-resolved".into()),
                payload_json: Set(payload),
                state: Set("pending".into()),
                attempts: Set(0),
                available_at: Set(now),
                lease_owner: Set(None),
                lease_until: Set(None),
                last_error: Set(None),
                created_at: Set(now),
                processed_at: Set(None),
            })
            .on_conflict(
                OnConflict::column(outbox::Column::DedupeKey)
                    .do_nothing()
                    .to_owned(),
            )
            .exec_without_returning(&tx)
            .await
            .map_err(sql)?;
            tx.commit().await.map_err(sql)?;
            Ok(resolved as usize)
        })
    }
}
