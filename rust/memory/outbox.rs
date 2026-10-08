//! Events memory publishes for other consumers, delivered at most once per
//! consumer through `memory_processed_events`.

use super::store::entities::{outbox, processed_event};
use super::worker::DEFAULT_LEASE_MS;
use super::{redacted, MemoryStore};
use crate::fleet::{run_db, sql};
use sea_orm::sea_query::{Expr, LockBehavior, LockType, OnConflict};
use sea_orm::{
    ActiveValue::Set, ColumnTrait, EntityTrait, QueryFilter, QueryOrder, QuerySelect,
    TransactionTrait,
};
use serde_json::Value;

#[derive(Debug, Clone)]
pub struct OutboxEvent {
    pub id: String,
    pub dedupe_key: String,
    pub kind: String,
    pub payload: Value,
}

pub trait OutboxConsumer {
    fn name(&self) -> &str;
    fn consume(&self, event: &OutboxEvent) -> Result<(), String>;
}

impl MemoryStore {
    pub fn enqueue_outbox(
        &self,
        dedupe_key: &str,
        kind: &str,
        payload: &Value,
    ) -> Result<String, String> {
        let now = super::now_ms();
        let dedupe_key = dedupe_key.to_owned();
        let row = outbox::ActiveModel {
            id: Set(super::stable_id("evt")),
            dedupe_key: Set(dedupe_key.clone()),
            event_kind: Set(kind.to_owned()),
            payload_json: Set(serde_json::to_string(payload).map_err(|e| e.to_string())?),
            state: Set("pending".into()),
            attempts: Set(0),
            available_at: Set(now),
            lease_owner: Set(None),
            lease_until: Set(None),
            last_error: Set(None),
            created_at: Set(now),
            processed_at: Set(None),
        };
        run_db(move |db| async move {
            outbox::Entity::insert(row)
                .on_conflict(
                    OnConflict::column(outbox::Column::DedupeKey)
                        .do_nothing()
                        .to_owned(),
                )
                .exec_without_returning(&db)
                .await
                .map_err(sql)?;
            outbox::Entity::find()
                .filter(outbox::Column::DedupeKey.eq(dedupe_key))
                .one(&db)
                .await
                .map_err(sql)?
                .map(|event| event.id)
                .ok_or_else(|| "the outbox event vanished after it was written".to_owned())
        })
    }

    pub fn consume_outbox_one(&self, consumer: &dyn OutboxConsumer) -> Result<bool, String> {
        let now = super::now_ms();
        let name = consumer.name().to_owned();
        let owner = name.clone();
        // Ok(event) to deliver, or Err(outcome): false when nothing was
        // pending, true when this consumer had already processed the event.
        let claimed: Result<outbox::Model, bool> = run_db(move |db| async move {
            let tx = db.begin().await.map_err(sql)?;
            let Some(event) = outbox::Entity::find()
                .filter(outbox::Column::State.eq("pending"))
                .filter(outbox::Column::AvailableAt.lte(now))
                .order_by_asc(outbox::Column::CreatedAt)
                .limit(1)
                .lock_with_behavior(LockType::Update, LockBehavior::SkipLocked)
                .one(&tx)
                .await
                .map_err(sql)?
            else {
                tx.commit().await.map_err(sql)?;
                return Ok(Err(false));
            };
            let already = processed_event::Entity::find_by_id((owner.clone(), event.id.clone()))
                .one(&tx)
                .await
                .map_err(sql)?
                .is_some();
            let update =
                outbox::Entity::update_many().filter(outbox::Column::Id.eq(event.id.clone()));
            if already {
                update
                    .col_expr(outbox::Column::State, Expr::value("done"))
                    .col_expr(outbox::Column::ProcessedAt, Expr::value(now))
                    .exec(&tx)
                    .await
                    .map_err(sql)?;
                tx.commit().await.map_err(sql)?;
                return Ok(Err(true));
            }
            update
                .col_expr(outbox::Column::State, Expr::value("processing"))
                .col_expr(outbox::Column::Attempts, Expr::value(event.attempts + 1))
                .col_expr(outbox::Column::LeaseOwner, Expr::value(owner))
                .col_expr(
                    outbox::Column::LeaseUntil,
                    Expr::value(now + DEFAULT_LEASE_MS),
                )
                .exec(&tx)
                .await
                .map_err(sql)?;
            tx.commit().await.map_err(sql)?;
            Ok(Ok(event))
        })?;
        let row = match claimed {
            Ok(event) => event,
            Err(outcome) => return Ok(outcome),
        };
        let event = OutboxEvent {
            id: row.id.clone(),
            dedupe_key: row.dedupe_key,
            kind: row.event_kind,
            payload: serde_json::from_str(&row.payload_json).map_err(|e| e.to_string())?,
        };
        let id = row.id;
        if let Err(error) = consumer.consume(&event) {
            let detail = redacted(&error);
            run_db(move |db| async move {
                outbox::Entity::update_many()
                    .col_expr(outbox::Column::State, Expr::value("pending"))
                    .col_expr(
                        outbox::Column::LeaseOwner,
                        Expr::value(Option::<String>::None),
                    )
                    .col_expr(outbox::Column::LeaseUntil, Expr::value(Option::<i64>::None))
                    .col_expr(outbox::Column::LastError, Expr::value(detail))
                    .col_expr(outbox::Column::AvailableAt, Expr::value(now + 1_000))
                    .filter(outbox::Column::Id.eq(id))
                    .exec(&db)
                    .await
                    .map_err(sql)?;
                Ok(())
            })?;
            return Err(error);
        }
        run_db(move |db| async move {
            let tx = db.begin().await.map_err(sql)?;
            let at = super::now_ms();
            processed_event::Entity::insert(processed_event::ActiveModel {
                consumer: Set(name),
                event_id: Set(id.clone()),
                processed_at: Set(at),
            })
            .on_conflict(
                OnConflict::columns([
                    processed_event::Column::Consumer,
                    processed_event::Column::EventId,
                ])
                .do_nothing()
                .to_owned(),
            )
            .exec_without_returning(&tx)
            .await
            .map_err(sql)?;
            outbox::Entity::update_many()
                .col_expr(outbox::Column::State, Expr::value("done"))
                .col_expr(
                    outbox::Column::LeaseOwner,
                    Expr::value(Option::<String>::None),
                )
                .col_expr(outbox::Column::LeaseUntil, Expr::value(Option::<i64>::None))
                .col_expr(outbox::Column::ProcessedAt, Expr::value(at))
                .filter(outbox::Column::Id.eq(id))
                .exec(&tx)
                .await
                .map_err(sql)?;
            tx.commit().await.map_err(sql)?;
            Ok(true)
        })
    }
}
