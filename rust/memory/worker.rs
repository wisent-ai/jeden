use super::{bounded_redacted, LeasedJob, MemoryScope, MemorySource, MemoryStore};
use rusqlite::{params, OptionalExtension, TransactionBehavior};
use serde_json::Value;

pub(super) const DEFAULT_LEASE_MS: i64 = 30_000;
pub const MAX_ATTEMPTS: i64 = 5;

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
    pub fn enqueue(&self, kind: &str, payload: &Value) -> Result<String, String> {
        let conn = self.connect()?;
        let id = super::stable_id("job");
        let now = super::now_ms();
        conn.execute("INSERT INTO jobs(id,kind,payload_json,state,available_at,created_at,updated_at) VALUES(?1,?2,?3,'queued',?4,?4,?4)", params![id,kind,serde_json::to_string(payload).map_err(|e|e.to_string())?,now]).map_err(|e|e.to_string())?;
        Ok(id)
    }

    pub fn claim(&self, worker: &str, lease_ms: Option<i64>) -> Result<Option<LeasedJob>, String> {
        let now = super::now_ms();
        let until = now + lease_ms.unwrap_or(DEFAULT_LEASE_MS).clamp(1_000, 300_000);
        let mut conn = self.connect()?;
        let tx = conn
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(|e| e.to_string())?;
        let id:Option<String>=tx.query_row("SELECT id FROM jobs WHERE attempts < ?1 AND available_at<=?2 AND (state='queued' OR (state='leased' AND lease_until<?2)) ORDER BY created_at LIMIT 1",params![MAX_ATTEMPTS,now],|r|r.get(0)).optional().map_err(|e|e.to_string())?;
        let Some(id) = id else {
            tx.commit().map_err(|e| e.to_string())?;
            return Ok(None);
        };
        tx.execute("UPDATE jobs SET state='leased',lease_owner=?2,lease_until=?3,heartbeat_at=?4,attempts=attempts+1,updated_at=?4 WHERE id=?1",params![id,worker,until,now]).map_err(|e|e.to_string())?;
        let job = tx
            .query_row(
                "SELECT id,kind,payload_json,attempts,lease_until FROM jobs WHERE id=?1",
                [&id],
                |r| {
                    let raw: String = r.get(2)?;
                    Ok(LeasedJob {
                        id: r.get(0)?,
                        kind: r.get(1)?,
                        payload: serde_json::from_str(&raw).unwrap_or(Value::Null),
                        attempts: r.get(3)?,
                        lease_until: r.get(4)?,
                    })
                },
            )
            .map_err(|e| e.to_string())?;
        tx.commit().map_err(|e| e.to_string())?;
        Ok(Some(job))
    }

    pub fn heartbeat(&self, id: &str, worker: &str, lease_ms: i64) -> Result<bool, String> {
        let now = super::now_ms();
        let conn = self.connect()?;
        Ok(conn.execute("UPDATE jobs SET heartbeat_at=?3,lease_until=?4,updated_at=?3 WHERE id=?1 AND state='leased' AND lease_owner=?2",params![id,worker,now,now+lease_ms.clamp(1_000,300_000)]).map_err(|e|e.to_string())?==1)
    }
    pub fn complete(&self, id: &str, worker: &str) -> Result<bool, String> {
        let conn = self.connect()?;
        Ok(conn.execute("UPDATE jobs SET state='done',lease_owner=NULL,lease_until=NULL,updated_at=?3 WHERE id=?1 AND state='leased' AND lease_owner=?2",params![id,worker,super::now_ms()]).map_err(|e|e.to_string())?==1)
    }
    pub fn retry(&self, id: &str, worker: &str, error: &str) -> Result<bool, String> {
        let conn = self.connect()?;
        let now = super::now_ms();
        let attempts: i64 = conn
            .query_row("SELECT attempts FROM jobs WHERE id=?1", [id], |r| r.get(0))
            .map_err(|e| e.to_string())?;
        let state = if attempts >= MAX_ATTEMPTS {
            "failed"
        } else {
            "queued"
        };
        let delay = (1_i64 << attempts.min(8)) * 1_000;
        Ok(conn.execute("UPDATE jobs SET state=?3,available_at=?4,lease_owner=NULL,lease_until=NULL,last_error=?5,updated_at=?6 WHERE id=?1 AND lease_owner=?2",params![id,worker,state,now+delay,bounded_redacted(error,500),now]).map_err(|e|e.to_string())?==1)
    }

    pub fn process_one(&self, worker: &str) -> Result<bool, String> {
        let Some(job) = self.claim(worker, None)? else {
            return Ok(false);
        };
        if !self.heartbeat(&job.id, worker, DEFAULT_LEASE_MS)? {
            return Err("memory job lease was lost before processing".into());
        }
        let outcome = match job.kind.as_str() {
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
            other => Err(format!("unsupported memory job kind: {other}")),
        };
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

    pub fn enqueue_outbox(
        &self,
        dedupe_key: &str,
        kind: &str,
        payload: &Value,
    ) -> Result<String, String> {
        let conn = self.connect()?;
        let id = super::stable_id("evt");
        let now = super::now_ms();
        conn.execute("INSERT OR IGNORE INTO memory_outbox(id,dedupe_key,event_kind,payload_json,available_at,created_at) VALUES(?1,?2,?3,?4,?5,?5)",params![id,dedupe_key,kind,serde_json::to_string(payload).map_err(|e|e.to_string())?,now]).map_err(|e|e.to_string())?;
        conn.query_row(
            "SELECT id FROM memory_outbox WHERE dedupe_key=?1",
            [dedupe_key],
            |row| row.get(0),
        )
        .map_err(|e| e.to_string())
    }

    pub fn consume_outbox_one(&self, consumer: &dyn OutboxConsumer) -> Result<bool, String> {
        let now = super::now_ms();
        let mut conn = self.connect()?;
        let tx = conn
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(|e| e.to_string())?;
        let row:Option<(String,String,String,String)>=tx.query_row("SELECT id,dedupe_key,event_kind,payload_json FROM memory_outbox WHERE state='pending' AND available_at<=?1 ORDER BY created_at LIMIT 1",[now],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?,r.get(3)?))).optional().map_err(|e|e.to_string())?;
        let Some((id, dedupe_key, kind, raw)) = row else {
            tx.commit().map_err(|e| e.to_string())?;
            return Ok(false);
        };
        let already:bool=tx.query_row("SELECT EXISTS(SELECT 1 FROM memory_processed_events WHERE consumer=?1 AND event_id=?2)",params![consumer.name(),id],|r|r.get(0)).map_err(|e|e.to_string())?;
        if already {
            tx.execute(
                "UPDATE memory_outbox SET state='done',processed_at=?2 WHERE id=?1",
                params![id, now],
            )
            .map_err(|e| e.to_string())?;
            tx.commit().map_err(|e| e.to_string())?;
            return Ok(true);
        }
        tx.execute("UPDATE memory_outbox SET state='processing',attempts=attempts+1,lease_owner=?2,lease_until=?3 WHERE id=?1",params![id,consumer.name(),now+DEFAULT_LEASE_MS]).map_err(|e|e.to_string())?;
        tx.commit().map_err(|e| e.to_string())?;
        let event = OutboxEvent {
            id: id.clone(),
            dedupe_key,
            kind,
            payload: serde_json::from_str(&raw).map_err(|e| e.to_string())?,
        };
        if let Err(error) = consumer.consume(&event) {
            self.connect()?.execute("UPDATE memory_outbox SET state='pending',lease_owner=NULL,lease_until=NULL,last_error=?2,available_at=?3 WHERE id=?1",params![id,bounded_redacted(&error,500),now+1_000]).map_err(|e|e.to_string())?;
            return Err(error);
        }
        let mut conn = self.connect()?;
        let tx = conn
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(|e| e.to_string())?;
        tx.execute("INSERT OR IGNORE INTO memory_processed_events(consumer,event_id,processed_at) VALUES(?1,?2,?3)",params![consumer.name(),id,super::now_ms()]).map_err(|e|e.to_string())?;
        tx.execute("UPDATE memory_outbox SET state='done',lease_owner=NULL,lease_until=NULL,processed_at=?2 WHERE id=?1",params![id,super::now_ms()]).map_err(|e|e.to_string())?;
        tx.commit().map_err(|e| e.to_string())?;
        Ok(true)
    }
}
