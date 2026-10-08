use super::store::entities::job;
use super::{
    redacted, LeasedJob, MemoryQueueJob, MemoryQueueStatus, MemoryScope, MemorySource,
    MemoryStore,
};
use crate::fleet::{run_db, sql};
use sea_orm::sea_query::{Condition, Expr, Order, SimpleExpr};
use sea_orm::{
    ActiveValue::Set, ColumnTrait, EntityTrait, QueryFilter, QueryOrder, QuerySelect,
    TransactionTrait,
};
use serde_json::Value;

pub(super) const DEFAULT_LEASE_MS: i64 = 30_000;
pub const MAX_ATTEMPTS: i64 = 5;

/// Set `id`'s columns in `changes` when `id` is leased to `worker`;
/// true when the row was this worker's to change.
async fn update_leased(
    db: &sea_orm::DatabaseConnection,
    id: String,
    worker: String,
    changes: Vec<(job::Column, SimpleExpr)>,
    leased_only: bool,
) -> Result<bool, String> {
    let mut update = job::Entity::update_many();
    for (column, value) in changes {
        update = update.col_expr(column, value);
    }
    let mut update = update
        .filter(job::Column::Id.eq(id))
        .filter(job::Column::LeaseOwner.eq(worker));
    if leased_only {
        update = update.filter(job::Column::State.eq("leased"));
    }
    Ok(update.exec(db).await.map_err(sql)?.rows_affected == 1)
}

impl MemoryStore {
    pub fn enqueue(&self, kind: &str, payload: &Value) -> Result<String, String> {
        let id = super::stable_id("job");
        let now = super::now_ms();
        let row = job::ActiveModel {
            id: Set(id.clone()),
            kind: Set(kind.to_owned()),
            payload_json: Set(serde_json::to_string(payload).map_err(|e| e.to_string())?),
            state: Set("queued".into()),
            attempts: Set(0),
            available_at: Set(now),
            lease_owner: Set(None),
            lease_until: Set(None),
            heartbeat_at: Set(None),
            last_error: Set(None),
            created_at: Set(now),
            updated_at: Set(now),
        };
        run_db(move |db| async move {
            job::Entity::insert(row)
                .exec_without_returning(&db)
                .await
                .map_err(sql)?;
            Ok(())
        })?;
        Ok(id)
    }

    pub fn queue_status(&self, limit: usize) -> Result<MemoryQueueStatus, String> {
        let limit = limit.min(200) as u64;
        let (state_counts, rows) = run_db(move |db| async move {
            let counts = job::Entity::find()
                .select_only()
                .column(job::Column::State)
                .column_as(job::Column::Id.count(), "jobs")
                .group_by(job::Column::State)
                .into_tuple::<(String, i64)>()
                .all(&db)
                .await
                .map_err(sql)?
                .into_iter()
                .collect::<std::collections::BTreeMap<_, _>>();
            let rows = job::Entity::find()
                .order_by(
                    Expr::cust(
                        "CASE state WHEN 'leased' THEN 0 WHEN 'queued' THEN 1 WHEN 'failed' THEN 2 ELSE 3 END",
                    ),
                    Order::Asc,
                )
                .order_by_desc(job::Column::UpdatedAt)
                .order_by_asc(job::Column::Id)
                .limit(limit)
                .all(&db)
                .await
                .map_err(sql)?;
            Ok((counts, rows))
        })?;
        let jobs = rows
            .into_iter()
            .map(|row| MemoryQueueJob {
                id: row.id,
                kind: row.kind,
                state: row.state,
                attempts: row.attempts,
                available_at: row.available_at,
                lease_owner: row.lease_owner,
                lease_until: row.lease_until,
                last_error: row.last_error,
                created_at: row.created_at,
                updated_at: row.updated_at,
            })
            .collect();
        let queued = state_counts.get("queued").copied().unwrap_or_default();
        let leased = state_counts.get("leased").copied().unwrap_or_default();
        let done = state_counts.get("done").copied().unwrap_or_default();
        let failed = state_counts.get("failed").copied().unwrap_or_default();
        let total = state_counts.values().sum();
        Ok(MemoryQueueStatus {
            total,
            pending: queued + leased,
            queued,
            leased,
            done,
            failed,
            jobs,
        })
    }

    pub fn claim(&self, worker: &str, lease_ms: Option<i64>) -> Result<Option<LeasedJob>, String> {
        let now = super::now_ms();
        let until = now + lease_ms.unwrap_or(DEFAULT_LEASE_MS).clamp(1_000, 300_000);
        let worker = worker.to_owned();
        run_db(move |db| async move {
            let tx = db.begin().await.map_err(sql)?;
            // SKIP LOCKED lets workers on other hosts claim other jobs at once.
            let Some(found) = job::Entity::find()
                .filter(job::Column::Attempts.lt(MAX_ATTEMPTS))
                .filter(job::Column::AvailableAt.lte(now))
                .filter(
                    Condition::any().add(job::Column::State.eq("queued")).add(
                        Condition::all()
                            .add(job::Column::State.eq("leased"))
                            .add(job::Column::LeaseUntil.lt(now)),
                    ),
                )
                .order_by_asc(job::Column::CreatedAt)
                .limit(1)
                .lock_with_behavior(
                    sea_orm::sea_query::LockType::Update,
                    sea_orm::sea_query::LockBehavior::SkipLocked,
                )
                .one(&tx)
                .await
                .map_err(sql)?
            else {
                tx.commit().await.map_err(sql)?;
                return Ok(None);
            };
            let attempts = found.attempts + 1;
            job::Entity::update_many()
                .col_expr(job::Column::State, Expr::value("leased"))
                .col_expr(job::Column::LeaseOwner, Expr::value(worker))
                .col_expr(job::Column::LeaseUntil, Expr::value(until))
                .col_expr(job::Column::HeartbeatAt, Expr::value(now))
                .col_expr(job::Column::Attempts, Expr::value(attempts))
                .col_expr(job::Column::UpdatedAt, Expr::value(now))
                .filter(job::Column::Id.eq(found.id.clone()))
                .exec(&tx)
                .await
                .map_err(sql)?;
            tx.commit().await.map_err(sql)?;
            Ok(Some(LeasedJob {
                payload: serde_json::from_str(&found.payload_json).unwrap_or(Value::Null),
                id: found.id,
                kind: found.kind,
                attempts,
                lease_until: until,
            }))
        })
    }

    pub fn heartbeat(&self, id: &str, worker: &str, lease_ms: i64) -> Result<bool, String> {
        let now = super::now_ms();
        let until = now + lease_ms.clamp(1_000, 300_000);
        let (id, worker) = (id.to_owned(), worker.to_owned());
        run_db(move |db| async move {
            let changes = vec![
                (job::Column::HeartbeatAt, Expr::value(now)),
                (job::Column::LeaseUntil, Expr::value(until)),
                (job::Column::UpdatedAt, Expr::value(now)),
            ];
            update_leased(&db, id, worker, changes, true).await
        })
    }
    pub fn complete(&self, id: &str, worker: &str) -> Result<bool, String> {
        let (id, worker) = (id.to_owned(), worker.to_owned());
        let now = super::now_ms();
        run_db(move |db| async move {
            let changes = vec![
                (job::Column::State, Expr::value("done")),
                (job::Column::LeaseOwner, Expr::value(Option::<String>::None)),
                (job::Column::LeaseUntil, Expr::value(Option::<i64>::None)),
                (job::Column::UpdatedAt, Expr::value(now)),
            ];
            update_leased(&db, id, worker, changes, true).await
        })
    }
    pub fn retry(&self, id: &str, worker: &str, error: &str) -> Result<bool, String> {
        let now = super::now_ms();
        let (id, worker, error) = (
            id.to_owned(),
            worker.to_owned(),
            redacted(error),
        );
        run_db(move |db| async move {
            let attempts = job::Entity::find_by_id(id.clone())
                .one(&db)
                .await
                .map_err(sql)?
                .map(|row| row.attempts)
                .ok_or_else(|| format!("memory job {id} does not exist"))?;
            let state = if attempts >= MAX_ATTEMPTS {
                "failed"
            } else {
                "queued"
            };
            let delay = (1_i64 << attempts.min(8)) * 1_000;
            let changes = vec![
                (job::Column::State, Expr::value(state)),
                (job::Column::AvailableAt, Expr::value(now + delay)),
                (job::Column::LeaseOwner, Expr::value(Option::<String>::None)),
                (job::Column::LeaseUntil, Expr::value(Option::<i64>::None)),
                (job::Column::LastError, Expr::value(error)),
                (job::Column::UpdatedAt, Expr::value(now)),
            ];
            update_leased(&db, id, worker, changes, false).await
        })
    }

    pub fn process_one(&self, worker: &str) -> Result<bool, String> {
        let Some(job) = self.claim(worker, None)? else {
            return Ok(false);
        };
        if !self.heartbeat(&job.id, worker, DEFAULT_LEASE_MS)? {
            return Err("memory job lease was lost before processing".into());
        }
        let outcome = (|| -> Result<(), String> {
            match job.kind.as_str() {
                "extract" => {
                    let scope: MemoryScope = serde_json::from_value(
                        job.payload
                            .get("scope")
                            .cloned()
                            .ok_or("extract job missing scope")?,
                    )
                    .map_err(|e| e.to_string())?;
                    let text = job
                        .payload
                        .get("text")
                        .and_then(Value::as_str)
                        .ok_or("extract job missing text")?;
                    let source = MemorySource {
                        origin: "session_extraction".into(),
                        session_id: job
                            .payload
                            .get("sessionId")
                            .and_then(Value::as_str)
                            .map(str::to_string),
                        entry_id: job
                            .payload
                            .get("entryId")
                            .and_then(Value::as_str)
                            .map(str::to_string),
                    };
                    self.remember("session", &scope, text, &["automatic".into()], &source, 0.6)
                        .map(|_| ())
                }
                "reindex" | "rebuild" => self.rebuild_fts().map(|_| ()),
                other => Err(format!("unsupported memory job kind: {other}")),
            }
        })();
        match outcome {
            Ok(()) => {
                if !self.complete(&job.id, worker)? {
                    return Err("memory job lease was lost before completion".into());
                }
                Ok(true)
            }
            Err(error) => {
                self.retry(&job.id, worker, &error)?;
                Err(error)
            }
        }
    }
}
