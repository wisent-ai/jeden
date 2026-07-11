use super::*;
use rusqlite::{params, Connection, OptionalExtension, TransactionBehavior};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};
use std::time::Duration;

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

    pub fn remember(
        &self,
        kind: &str,
        scope: &MemoryScope,
        text: &str,
        tags: &[String],
        source: &MemorySource,
        confidence: f64,
    ) -> Result<MemoryRecord, String> {
        let redacted = bounded_redacted(text, MAX_MEMORY_CHARS);
        let logical_key = logical_key(kind, scope, &redacted);
        self.remember_with_key(
            kind,
            scope,
            &logical_key,
            &redacted,
            tags,
            source,
            confidence,
            None,
        )
    }

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
        let text = bounded_redacted(text, MAX_MEMORY_CHARS);
        if text.is_empty() {
            return Err("memory text is empty after redaction".into());
        }
        let logical_key = logical_key.trim();
        if logical_key.is_empty() {
            return Err("memory logical key is empty".into());
        }
        let now = now_ms();
        let valid_from = valid_from.unwrap_or(now);
        let mut conn = self.connect()?;
        let tx = conn
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(|e| e.to_string())?;
        let prior:Option<MemoryRecord>=tx.query_row(
            "SELECT id,kind,scope_kind,scope_id,text,tags_json,source_json,confidence,status,created_at,updated_at,logical_key,revision,valid_from,valid_to,supersedes,tombstone FROM memories WHERE scope_kind=?1 AND scope_id=?2 AND logical_key=?3 AND valid_to IS NULL ORDER BY revision DESC LIMIT 1",
            params![scope.kind,scope.id,logical_key],row_record).optional().map_err(|e|e.to_string())?;
        if let Some(existing) = prior
            .as_ref()
            .filter(|record| record.text == text && !record.tombstone && record.status == "active")
        {
            tx.commit().map_err(|e| e.to_string())?;
            return Ok(existing.clone());
        }
        let revision = prior
            .as_ref()
            .map(|record| record.revision + 1)
            .unwrap_or(1);
        let supersedes = prior.as_ref().map(|record| record.id.clone());
        if let Some(prior) = &prior {
            tx.execute(
                "UPDATE memories SET valid_to=?2,status='superseded',updated_at=?2 WHERE id=?1",
                params![prior.id, valid_from],
            )
            .map_err(|e| e.to_string())?;
        }
        let id = stable_id("mem");
        tx.execute("INSERT INTO memories(id,kind,scope_kind,scope_id,text,tags_json,source_json,confidence,status,created_at,updated_at,logical_key,revision,valid_from,valid_to,supersedes,tombstone) VALUES(?1,?2,?3,?4,?5,?6,?7,?8,'active',?9,?9,?10,?11,?12,NULL,?13,0)",params![id,kind,scope.kind,scope.id,text,serde_json::to_string(tags).map_err(|e|e.to_string())?,serde_json::to_string(source).map_err(|e|e.to_string())?,confidence.clamp(0.0,1.0),now,logical_key,revision,valid_from,supersedes]).map_err(|e|e.to_string())?;
        tx.execute("INSERT OR IGNORE INTO memory_outbox(id,dedupe_key,event_kind,payload_json,available_at,created_at) VALUES(?1,?2,'memory-upserted',?3,?4,?4)",params![stable_id("evt"),format!("memory-upserted:{id}"),json!({"memoryId":id}).to_string(),now]).map_err(|e|e.to_string())?;
        let record=tx.query_row("SELECT id,kind,scope_kind,scope_id,text,tags_json,source_json,confidence,status,created_at,updated_at,logical_key,revision,valid_from,valid_to,supersedes,tombstone FROM memories WHERE id=?1",[&id],row_record).map_err(|e|e.to_string())?;
        tx.commit().map_err(|e| e.to_string())?;
        Ok(record)
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
        let mut conn = self.connect()?;
        let tx = conn
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(|e| e.to_string())?;
        let prior:MemoryRecord=tx.query_row("SELECT id,kind,scope_kind,scope_id,text,tags_json,source_json,confidence,status,created_at,updated_at,logical_key,revision,valid_from,valid_to,supersedes,tombstone FROM memories WHERE scope_kind=?1 AND scope_id=?2 AND logical_key=?3 AND valid_to IS NULL ORDER BY revision DESC LIMIT 1",params![scope.kind,scope.id,logical_key],row_record).map_err(|_|"active logical memory not found".to_string())?;
        tx.execute(
            "UPDATE memories SET valid_to=?2,status='superseded',updated_at=?2 WHERE id=?1",
            params![prior.id, at],
        )
        .map_err(|e| e.to_string())?;
        let id = stable_id("mem");
        tx.execute("INSERT INTO memories(id,kind,scope_kind,scope_id,text,tags_json,source_json,confidence,status,created_at,updated_at,logical_key,revision,valid_from,supersedes,tombstone) VALUES(?1,?2,?3,?4,'','[]',?5,1.0,'active',?6,?6,?7,?8,?9,?10,1)",params![id,prior.kind,scope.kind,scope.id,serde_json::to_string(source).map_err(|e|e.to_string())?,now,logical_key,prior.revision+1,at,prior.id]).map_err(|e|e.to_string())?;
        let record=tx.query_row("SELECT id,kind,scope_kind,scope_id,text,tags_json,source_json,confidence,status,created_at,updated_at,logical_key,revision,valid_from,valid_to,supersedes,tombstone FROM memories WHERE id=?1",[&id],row_record).map_err(|e|e.to_string())?;
        tx.commit().map_err(|e| e.to_string())?;
        Ok(record)
    }

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

    pub fn forget_scope(&self, scope: &MemoryScope) -> Result<usize, String> {
        let conn = self.connect()?;
        conn.execute("UPDATE memories SET status='forgotten',valid_to=COALESCE(valid_to,?3),updated_at=?3 WHERE scope_kind=?1 AND scope_id=?2 AND status='active'",params![scope.kind,scope.id,now_ms()]).map_err(|e|e.to_string())
    }
    pub fn clear(&self) -> Result<usize, String> {
        self.connect()?
            .execute("DELETE FROM memories", [])
            .map_err(|e| e.to_string())
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

    pub fn health(&self) -> Result<Value, String> {
        let conn = self.connect()?;
        let memories:i64=conn.query_row("SELECT count(*) FROM memories WHERE status='active' AND tombstone=0 AND valid_to IS NULL",[],|r|r.get(0)).map_err(|e|e.to_string())?;
        let queued: i64 = conn
            .query_row(
                "SELECT count(*) FROM jobs WHERE state IN ('queued','leased')",
                [],
                |r| r.get(0),
            )
            .map_err(|e| e.to_string())?;
        let embedding = embeddings::health(&conn, None)?;
        Ok(
            json!({"service":"memory","healthy":true,"backend":"sqlite-wal-fts5","retrievalMode":embedding.mode,"embeddingAvailable":embedding.available,"schemaVersion":schema::SCHEMA_VERSION,"path":self.path,"activeMemories":memories,"pendingJobs":queued,"provenance":true,"bounded":{"memoryChars":MAX_MEMORY_CHARS,"contextChars":MAX_CONTEXT_CHARS,"attempts":MAX_ATTEMPTS}}),
        )
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
fn logical_key(kind: &str, scope: &MemoryScope, text: &str) -> String {
    let mut hash = Sha256::new();
    for part in [kind, scope.kind.as_str(), scope.id.as_str(), text] {
        hash.update(part.as_bytes());
        hash.update([0])
    }
    format!("auto:{}", hex::encode(hash.finalize()))
}
