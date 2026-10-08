//! Putting something into memory so the record of what was believed, and
//! when, survives being corrected.

use super::super::*;
use super::entities::{memory, outbox};
use super::{logical_key, record, MemoryStore};
use crate::fleet::{run_db, sql};
use sea_orm::sea_query::{Expr, OnConflict};
use sea_orm::{
    ActiveValue::Set, ColumnTrait, DatabaseTransaction, EntityTrait, QueryFilter, QueryOrder,
    QuerySelect, TransactionTrait,
};
use serde_json::json;

/// The live revision of `logical_key` in `scope`, locked for this write.
async fn live_revision(
    tx: &DatabaseTransaction,
    scope: &MemoryScope,
    logical_key: &str,
) -> Result<Option<MemoryRecord>, String> {
    Ok(memory::Entity::find()
        .filter(memory::Column::ScopeKind.eq(scope.kind.clone()))
        .filter(memory::Column::ScopeId.eq(scope.id.clone()))
        .filter(memory::Column::LogicalKey.eq(logical_key.to_owned()))
        .filter(memory::Column::ValidTo.is_null())
        .order_by_desc(memory::Column::Revision)
        .limit(1)
        .lock_exclusive()
        .one(tx)
        .await
        .map_err(sql)?
        .map(record))
}

/// Close `prior` at `at`; the next revision supersedes it.
async fn supersede(tx: &DatabaseTransaction, prior: &str, at: i64) -> Result<(), String> {
    memory::Entity::update_many()
        .col_expr(memory::Column::ValidTo, Expr::value(at))
        .col_expr(memory::Column::Status, Expr::value("superseded"))
        .col_expr(memory::Column::UpdatedAt, Expr::value(at))
        .filter(memory::Column::Id.eq(prior.to_owned()))
        .exec(tx)
        .await
        .map_err(sql)?;
    Ok(())
}

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
        let kept = redacted(text);
        let logical_key = logical_key(kind, scope, &kept);
        self.remember_with_key(
            kind,
            scope,
            &logical_key,
            &kept,
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
        let text = redacted(text);
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
        run_db(move |db| async move {
            let tx = db.begin().await.map_err(sql)?;
            let prior = live_revision(&tx, &scope, &logical_key).await?;
            if let Some(existing) = prior.as_ref().filter(|record| {
                record.text == text && !record.tombstone && record.status == "active"
            }) {
                tx.commit().await.map_err(sql)?;
                return Ok(existing.clone());
            }
            if let Some(prior) = &prior {
                supersede(&tx, &prior.id, valid_from).await?;
            }
            let id = stable_id("mem");
            let row = memory::ActiveModel {
                id: Set(id.clone()),
                kind: Set(kind),
                scope_kind: Set(scope.kind),
                scope_id: Set(scope.id),
                text: Set(text),
                tags_json: Set(tags),
                source_json: Set(source),
                confidence: Set(confidence),
                status: Set("active".into()),
                created_at: Set(now),
                updated_at: Set(now),
                logical_key: Set(logical_key),
                revision: Set(prior
                    .as_ref()
                    .map(|record| record.revision + 1)
                    .unwrap_or(1)),
                valid_from: Set(valid_from),
                valid_to: Set(None),
                supersedes: Set(prior.map(|record| record.id)),
                tombstone: Set(false),
            };
            let written = memory::Entity::insert(row)
                .exec_with_returning(&tx)
                .await
                .map_err(sql)?;
            outbox::Entity::insert(outbox::ActiveModel {
                id: Set(stable_id("evt")),
                dedupe_key: Set(format!("memory-upserted:{id}")),
                event_kind: Set("memory-upserted".into()),
                payload_json: Set(json!({"memoryId": id}).to_string()),
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
            Ok(record(written))
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
        run_db(move |db| async move {
            let tx = db.begin().await.map_err(sql)?;
            let prior = live_revision(&tx, &scope, &logical_key)
                .await?
                .ok_or("active logical memory not found")?;
            supersede(&tx, &prior.id, at).await?;
            let row = memory::ActiveModel {
                id: Set(stable_id("mem")),
                kind: Set(prior.kind),
                scope_kind: Set(scope.kind),
                scope_id: Set(scope.id),
                text: Set(String::new()),
                tags_json: Set("[]".into()),
                source_json: Set(source),
                confidence: Set(1.0),
                status: Set("active".into()),
                created_at: Set(now),
                updated_at: Set(now),
                logical_key: Set(logical_key),
                revision: Set(prior.revision + 1),
                valid_from: Set(at),
                valid_to: Set(None),
                supersedes: Set(Some(prior.id)),
                tombstone: Set(true),
            };
            let written = memory::Entity::insert(row)
                .exec_with_returning(&tx)
                .await
                .map_err(sql)?;
            tx.commit().await.map_err(sql)?;
            Ok(record(written))
        })
    }

    pub fn forget_scope(&self, scope: &MemoryScope) -> Result<usize, String> {
        let scope = scope.clone();
        let now = now_ms();
        run_db(move |db| async move {
            let result = memory::Entity::update_many()
                .col_expr(memory::Column::Status, Expr::value("forgotten"))
                .col_expr(
                    memory::Column::ValidTo,
                    Expr::cust_with_values("COALESCE(valid_to, $1)", [now]),
                )
                .col_expr(memory::Column::UpdatedAt, Expr::value(now))
                .filter(memory::Column::ScopeKind.eq(scope.kind))
                .filter(memory::Column::ScopeId.eq(scope.id))
                .filter(memory::Column::Status.eq("active"))
                .exec(&db)
                .await
                .map_err(sql)?;
            Ok(result.rows_affected as usize)
        })
    }
    pub fn clear(&self) -> Result<usize, String> {
        run_db(|db| async move {
            let tx = db.begin().await.map_err(sql)?;
            // Revisions point at the rows they supersede; unlink them first.
            memory::Entity::update_many()
                .col_expr(
                    memory::Column::Supersedes,
                    Expr::value(Option::<String>::None),
                )
                .exec(&tx)
                .await
                .map_err(sql)?;
            let removed = memory::Entity::delete_many()
                .exec(&tx)
                .await
                .map_err(sql)?
                .rows_affected;
            tx.commit().await.map_err(sql)?;
            Ok(removed as usize)
        })
    }
}
