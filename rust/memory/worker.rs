use super::{
    bounded_redacted, LeasedJob, MemoryQueueJob, MemoryQueueStatus, MemoryScope, MemorySource,
    MemoryStore,
};
use crate::fleet::{run_db, sql};
use serde_json::Value;

pub(super) const DEFAULT_LEASE_MS: i64 = 30_000;
pub const MAX_ATTEMPTS: i64 = 5;

impl MemoryStore {
    pub fn enqueue(&self, kind: &str, payload: &Value) -> Result<String, String> {
        let id = super::stable_id("job");
        let now = super::now_ms();
        let (kind, payload) = (
            kind.to_owned(),
            serde_json::to_string(payload).map_err(|e| e.to_string())?,
        );
        let job = id.clone();
        run_db(move |client| {
            client
                .execute(
                    "INSERT INTO memory_jobs(id,kind,payload_json,state,available_at,created_at,updated_at)
                     VALUES($1,$2,$3,'queued',$4,$4,$4)",
                    &[&job, &kind, &payload, &now],
                )
                .map_err(sql)?;
            Ok(())
        })?;
        Ok(id)
    }

    pub fn queue_status(&self, limit: usize) -> Result<MemoryQueueStatus, String> {
        let limit = limit.min(200) as i64;
        let (state_counts, jobs) = run_db(move |client| {
            let counts = client
                .query("SELECT state,count(*) FROM memory_jobs GROUP BY state", &[])
                .map_err(sql)?
                .into_iter()
                .map(|row| (row.get::<_, String>(0), row.get::<_, i64>(1)))
                .collect::<std::collections::BTreeMap<_, _>>();
            let jobs = client
                .query(
                    "SELECT id,kind,state,attempts,available_at,lease_owner,lease_until,last_error,created_at,updated_at
                     FROM memory_jobs ORDER BY CASE state WHEN 'leased' THEN 0 WHEN 'queued' THEN 1
                     WHEN 'failed' THEN 2 ELSE 3 END,updated_at DESC,id LIMIT $1",
                    &[&limit],
                )
                .map_err(sql)?
                .into_iter()
                .map(|row| MemoryQueueJob {
                    id: row.get(0),
                    kind: row.get(1),
                    state: row.get(2),
                    attempts: row.get(3),
                    available_at: row.get(4),
                    lease_owner: row.get(5),
                    lease_until: row.get(6),
                    last_error: row.get(7),
                    created_at: row.get(8),
                    updated_at: row.get(9),
                })
                .collect::<Vec<_>>();
            Ok((counts, jobs))
        })?;
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
        run_db(move |client| {
            // SKIP LOCKED lets workers on other hosts claim other jobs at once.
            let row = client
                .query_opt(
                    "UPDATE memory_jobs SET state='leased',lease_owner=$3,lease_until=$4,heartbeat_at=$2,
                     attempts=attempts+1,updated_at=$2
                     WHERE id=(SELECT id FROM memory_jobs WHERE attempts<$1 AND available_at<=$2
                        AND (state='queued' OR (state='leased' AND lease_until<$2))
                        ORDER BY created_at LIMIT 1 FOR UPDATE SKIP LOCKED)
                     RETURNING id,kind,payload_json,attempts,lease_until",
                    &[&MAX_ATTEMPTS, &now, &worker, &until],
                )
                .map_err(sql)?;
            Ok(row.map(|row| {
                let raw: String = row.get(2);
                LeasedJob {
                    id: row.get(0),
                    kind: row.get(1),
                    payload: serde_json::from_str(&raw).unwrap_or(Value::Null),
                    attempts: row.get(3),
                    lease_until: row.get(4),
                }
            }))
        })
    }

    pub fn heartbeat(&self, id: &str, worker: &str, lease_ms: i64) -> Result<bool, String> {
        let now = super::now_ms();
        let until = now + lease_ms.clamp(1_000, 300_000);
        let (id, worker) = (id.to_owned(), worker.to_owned());
        run_db(move |client| {
            Ok(client
                .execute(
                    "UPDATE memory_jobs SET heartbeat_at=$3,lease_until=$4,updated_at=$3
                     WHERE id=$1 AND state='leased' AND lease_owner=$2",
                    &[&id, &worker, &now, &until],
                )
                .map_err(sql)?
                == 1)
        })
    }
    pub fn complete(&self, id: &str, worker: &str) -> Result<bool, String> {
        let (id, worker) = (id.to_owned(), worker.to_owned());
        run_db(move |client| {
            Ok(client
                .execute(
                    "UPDATE memory_jobs SET state='done',lease_owner=NULL,lease_until=NULL,updated_at=$3
                     WHERE id=$1 AND state='leased' AND lease_owner=$2",
                    &[&id, &worker, &super::now_ms()],
                )
                .map_err(sql)?
                == 1)
        })
    }
    pub fn retry(&self, id: &str, worker: &str, error: &str) -> Result<bool, String> {
        let now = super::now_ms();
        let (id, worker, error) = (
            id.to_owned(),
            worker.to_owned(),
            bounded_redacted(error, 500),
        );
        run_db(move |client| {
            let attempts: i64 = client
                .query_one("SELECT attempts FROM memory_jobs WHERE id=$1", &[&id])
                .map_err(sql)?
                .get(0);
            let state = if attempts >= MAX_ATTEMPTS {
                "failed"
            } else {
                "queued"
            };
            let delay = (1_i64 << attempts.min(8)) * 1_000;
            Ok(client
                .execute(
                    "UPDATE memory_jobs SET state=$3,available_at=$4,lease_owner=NULL,lease_until=NULL,
                     last_error=$5,updated_at=$6 WHERE id=$1 AND lease_owner=$2",
                    &[&id, &worker, &state, &(now + delay), &error, &now],
                )
                .map_err(sql)?
                == 1)
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
