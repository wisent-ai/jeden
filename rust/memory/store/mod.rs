use super::*;
use rusqlite::{params, Connection, OptionalExtension, TransactionBehavior};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};
use std::time::Duration;
mod maintenance;
mod recall;
mod writes;


pub struct MemoryStore {
    pub(super) path: PathBuf,
}

impl MemoryStore {
    pub fn default_path() -> PathBuf {
        std::env::var_os("JEDEN_MEMORY_DB")
            .map(PathBuf::from)
            .or_else(|| std::env::var_os("JEDEN_MEMORY_FILE").map(PathBuf::from))
            .unwrap_or_else(|| {
                PathBuf::from(std::env::var_os("HOME").unwrap_or_else(|| ".".into()))
                    .join(".jeden/memory.sqlite3")
            })
    }

    pub fn open(path: impl AsRef<Path>) -> Result<Self, String> {
        let path = path.as_ref().to_path_buf();
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(|e| e.to_string())?
        }
        let store = Self { path };
        let conn = store.connect()?;
        schema::initialize(&conn)?;
        drop(conn);
        schema::migrate(&store.path)?;
        Ok(store)
    }

    pub(crate) fn connect(&self) -> Result<Connection, String> {
        let conn = Connection::open(&self.path).map_err(|e| e.to_string())?;
        conn.busy_timeout(Duration::from_secs(10))
            .map_err(|e| e.to_string())?;
        conn.execute_batch("PRAGMA foreign_keys=ON;")
            .map_err(|e| e.to_string())?;
        Ok(conn)
    }
    pub fn path(&self) -> &Path {
        &self.path
    }


    pub fn embedding_health(
        &self,
        provider: Option<&dyn EmbeddingProvider>,
    ) -> Result<EmbeddingHealth, String> {
        embeddings::health(&self.connect()?, provider)
    }
    pub fn edges(&self, id: &str) -> Result<Vec<MemoryEdge>, String> {
        conflict::edges(&self.connect()?, id)
    }
    pub fn rebuild_embeddings(&self, provider: &dyn EmbeddingProvider) -> Result<usize, String> {
        embeddings::rebuild(&mut self.connect()?, provider)
    }

    pub fn acquire_scope_lock(
        &self,
        scope: &MemoryScope,
        owner: &str,
        ttl_ms: i64,
    ) -> Result<bool, String> {
        let now = now_ms();
        let mut conn = self.connect()?;
        let tx = conn
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(|e| e.to_string())?;
        tx.execute("DELETE FROM scope_locks WHERE expires_at<?1", [now])
            .map_err(|e| e.to_string())?;
        let acquired=tx.execute("INSERT INTO scope_locks(scope_kind,scope_id,owner,expires_at) VALUES(?1,?2,?3,?4) ON CONFLICT(scope_kind,scope_id) DO UPDATE SET owner=excluded.owner,expires_at=excluded.expires_at WHERE scope_locks.owner=excluded.owner",params![scope.kind,scope.id,owner,now+ttl_ms.clamp(1_000,300_000)]).map_err(|e|e.to_string())?==1;
        tx.commit().map_err(|e| e.to_string())?;
        Ok(acquired)
    }
    pub fn release_scope_lock(&self, scope: &MemoryScope, owner: &str) -> Result<(), String> {
        self.connect()?
            .execute(
                "DELETE FROM scope_locks WHERE scope_kind=?1 AND scope_id=?2 AND owner=?3",
                params![scope.kind, scope.id, owner],
            )
            .map_err(|e| e.to_string())?;
        Ok(())
    }

    pub fn pre_compaction_context(
        &self,
        scope: &MemoryScope,
        query: &str,
        max_chars: usize,
    ) -> Result<String, String> {
        let hits = self.recall(&FtsBackend, scope, query, 100)?;
        let cap = max_chars.min(MAX_CONTEXT_CHARS);
        let mut out = String::new();
        for hit in hits {
            let line = format!(
                "[{}; {}; {}] {}\n",
                hit.record.id, hit.provenance.backend, hit.record.source.origin, hit.record.text
            );
            if out.chars().count() + line.chars().count() > cap {
                break;
            }
            out.push_str(&line)
        }
        Ok(out)
    }
    pub fn consolidate(
        &self,
        scope: &MemoryScope,
        model: &dyn Consolidator,
        max_chars: usize,
    ) -> Result<MemoryRecord, String> {
        let candidates = self
            .recall(&FtsBackend, scope, "", 100)?
            .into_iter()
            .map(|h| h.record)
            .collect::<Vec<_>>();
        if candidates.len() < 2 {
            return Err("consolidation requires at least two memories".into());
        }
        let text = bounded_redacted(
            &model.consolidate(&candidates, max_chars.min(MAX_MEMORY_CHARS))?,
            max_chars.min(MAX_MEMORY_CHARS),
        );
        self.remember(
            "summary",
            scope,
            &text,
            &["consolidated".into()],
            &MemorySource {
                origin: "model_consolidation".into(),
                session_id: None,
                entry_id: None,
            },
            0.7,
        )
    }
    pub fn persist_model_consolidation(
        &self,
        scope: &MemoryScope,
        summary: &str,
    ) -> Result<MemoryRecord, String> {
        self.remember(
            "summary",
            scope,
            summary,
            &["consolidated".into(), "model-assisted".into()],
            &MemorySource {
                origin: "model_compaction".into(),
                session_id: None,
                entry_id: None,
            },
            0.85,
        )
    }

    pub fn record_workflow(
        &self,
        fingerprint: &str,
        description: &str,
        session_id: &str,
        verified: bool,
    ) -> Result<Option<String>, String> {
        let mut conn = self.connect()?;
        let tx = conn
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(|e| e.to_string())?;
        let prior: Option<(String, i64, i64)> = tx
            .query_row(
                "SELECT sessions_json,occurrences,verified FROM workflows WHERE fingerprint=?1",
                [fingerprint],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            )
            .optional()
            .map_err(|e| e.to_string())?;
        let (mut sessions, occ, was_verified) = prior
            .map(|(j, o, v)| {
                (
                    serde_json::from_str::<Vec<String>>(&j).unwrap_or_default(),
                    o,
                    v,
                )
            })
            .unwrap_or((Vec::new(), 0, 0));
        if !sessions.iter().any(|s| s == session_id) {
            sessions.push(session_id.into())
        }
        let occurrences = occ + 1;
        let is_verified = was_verified == 1 || verified;
        tx.execute("INSERT INTO workflows(fingerprint,description,sessions_json,occurrences,verified,updated_at) VALUES(?1,?2,?3,?4,?5,?6) ON CONFLICT(fingerprint) DO UPDATE SET description=excluded.description,sessions_json=excluded.sessions_json,occurrences=excluded.occurrences,verified=excluded.verified,updated_at=excluded.updated_at",params![fingerprint,bounded_redacted(description,MAX_MEMORY_CHARS),serde_json::to_string(&sessions).map_err(|e|e.to_string())?,occurrences,is_verified as i64,now_ms()]).map_err(|e|e.to_string())?;
        let skill = if occurrences >= 3 && is_verified {
            let id = format!(
                "skill_{}",
                &hex::encode(Sha256::digest(fingerprint.as_bytes()))[..24]
            );
            let body = format!(
                "# Managed workflow\n\n{}",
                bounded_redacted(description, MAX_MEMORY_CHARS)
            );
            tx.execute("INSERT OR IGNORE INTO managed_skills(id,workflow_fingerprint,body,created_at) VALUES(?1,?2,?3,?4)",params![id,fingerprint,body,now_ms()]).map_err(|e|e.to_string())?;
            Some(id)
        } else {
            None
        };
        tx.commit().map_err(|e| e.to_string())?;
        Ok(skill)
    }

}
pub(super) fn load_record(conn: &Connection, id: &str) -> Result<Option<MemoryRecord>, String> {
    conn.query_row("SELECT id,kind,scope_kind,scope_id,text,tags_json,source_json,confidence,status,created_at,updated_at,logical_key,revision,valid_from,valid_to,supersedes,tombstone FROM memories WHERE id=?1",[id],row_record).optional().map_err(|e|e.to_string())
}
pub(super) fn row_record(row: &rusqlite::Row<'_>) -> rusqlite::Result<MemoryRecord> {
    let tags: String = row.get(5)?;
    let source: String = row.get(6)?;
    Ok(MemoryRecord {
        id: row.get(0)?,
        kind: row.get(1)?,
        scope: MemoryScope {
            kind: row.get(2)?,
            id: row.get(3)?,
        },
        text: row.get(4)?,
        tags: serde_json::from_str(&tags).unwrap_or_default(),
        source: serde_json::from_str(&source).unwrap_or(MemorySource {
            origin: "unknown".into(),
            session_id: None,
            entry_id: None,
        }),
        confidence: row.get(7)?,
        status: row.get(8)?,
        created_at: row.get(9)?,
        updated_at: row.get(10)?,
        logical_key: row.get(11)?,
        revision: row.get(12)?,
        valid_from: row.get(13)?,
        valid_to: row.get(14)?,
        supersedes: row.get(15)?,
        tombstone: row.get::<_, i64>(16)? != 0,
    })
}
pub(super) fn logical_key(kind: &str, scope: &MemoryScope, text: &str) -> String {
    let mut hash = Sha256::new();
    for part in [kind, scope.kind.as_str(), scope.id.as_str(), text] {
        hash.update(part.as_bytes());
        hash.update([0])
    }
    format!("auto:{}", hex::encode(hash.finalize()))
}
