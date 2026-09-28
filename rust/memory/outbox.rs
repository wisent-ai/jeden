//! Events memory publishes for other consumers, delivered at most once per
//! consumer through `memory_processed_events`.

use super::worker::DEFAULT_LEASE_MS;
use super::{bounded_redacted, MemoryStore};
use crate::fleet::{run_db, sql};
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
        let id = super::stable_id("evt");
        let now = super::now_ms();
        let (dedupe_key, kind) = (dedupe_key.to_owned(), kind.to_owned());
        let payload = serde_json::to_string(payload).map_err(|e| e.to_string())?;
        run_db(move |client| {
            client
                .execute(
                    "INSERT INTO memory_outbox(id,dedupe_key,event_kind,payload_json,available_at,created_at)
                     VALUES($1,$2,$3,$4,$5,$5) ON CONFLICT DO NOTHING",
                    &[&id, &dedupe_key, &kind, &payload, &now],
                )
                .map_err(sql)?;
            Ok(client
                .query_one(
                    "SELECT id FROM memory_outbox WHERE dedupe_key=$1",
                    &[&dedupe_key],
                )
                .map_err(sql)?
                .get(0))
        })
    }

    pub fn consume_outbox_one(&self, consumer: &dyn OutboxConsumer) -> Result<bool, String> {
        let now = super::now_ms();
        let name = consumer.name().to_owned();
        let owner = name.clone();
        // Ok(event) to deliver, or Err(outcome): false when nothing was
        // pending, true when this consumer had already processed the event.
        let claimed: Result<(String, String, String, String), bool> = run_db(move |client| {
            let mut tx = client.transaction().map_err(sql)?;
            let Some(row) = tx
                .query_opt(
                    "SELECT id,dedupe_key,event_kind,payload_json FROM memory_outbox
                     WHERE state='pending' AND available_at<=$1 ORDER BY created_at LIMIT 1
                     FOR UPDATE SKIP LOCKED",
                    &[&now],
                )
                .map_err(sql)?
            else {
                tx.commit().map_err(sql)?;
                return Ok(Err(false));
            };
            let id: String = row.get(0);
            let already: bool = tx
                .query_one(
                    "SELECT EXISTS(SELECT 1 FROM memory_processed_events WHERE consumer=$1 AND event_id=$2)",
                    &[&owner, &id],
                )
                .map_err(sql)?
                .get(0);
            if already {
                tx.execute(
                    "UPDATE memory_outbox SET state='done',processed_at=$2 WHERE id=$1",
                    &[&id, &now],
                )
                .map_err(sql)?;
                tx.commit().map_err(sql)?;
                return Ok(Err(true));
            }
            tx.execute(
                "UPDATE memory_outbox SET state='processing',attempts=attempts+1,lease_owner=$2,lease_until=$3 WHERE id=$1",
                &[&id, &owner, &(now + DEFAULT_LEASE_MS)],
            )
            .map_err(sql)?;
            tx.commit().map_err(sql)?;
            Ok(Ok((id, row.get(1), row.get(2), row.get(3))))
        })?;
        let (id, dedupe_key, kind, raw) = match claimed {
            Ok(event) => event,
            Err(outcome) => return Ok(outcome),
        };
        let event = OutboxEvent {
            id: id.clone(),
            dedupe_key,
            kind,
            payload: serde_json::from_str(&raw).map_err(|e| e.to_string())?,
        };
        if let Err(error) = consumer.consume(&event) {
            let detail = bounded_redacted(&error, 500);
            run_db(move |client| {
                client
                    .execute(
                        "UPDATE memory_outbox SET state='pending',lease_owner=NULL,lease_until=NULL,
                         last_error=$2,available_at=$3 WHERE id=$1",
                        &[&id, &detail, &(now + 1_000)],
                    )
                    .map_err(sql)?;
                Ok(())
            })?;
            return Err(error);
        }
        run_db(move |client| {
            let mut tx = client.transaction().map_err(sql)?;
            let at = super::now_ms();
            tx.execute(
                "INSERT INTO memory_processed_events(consumer,event_id,processed_at) VALUES($1,$2,$3)
                 ON CONFLICT DO NOTHING",
                &[&name, &id, &at],
            )
            .map_err(sql)?;
            tx.execute(
                "UPDATE memory_outbox SET state='done',lease_owner=NULL,lease_until=NULL,processed_at=$2 WHERE id=$1",
                &[&id, &at],
            )
            .map_err(sql)?;
            tx.commit().map_err(sql)?;
            Ok(true)
        })
    }
}
