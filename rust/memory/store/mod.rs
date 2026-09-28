use super::*;
use crate::fleet::{run_db, sql};
use postgres::GenericClient;
use sha2::{Digest, Sha256};
mod maintenance;
mod recall;
mod writes;

/// The columns every memory read returns, in the order `row_record` reads.
pub(super) const RECORD_COLUMNS: &str = "id,kind,scope_kind,scope_id,text,tags_json,source_json,confidence,status,created_at,updated_at,logical_key,revision,valid_from,valid_to,supersedes,tombstone";

/// Jeden's memory, kept in the fleet database `jeden` so every host and
/// session of the operator reads and writes the same memories.
pub struct MemoryStore;

impl MemoryStore {
    /// Opens memory by reaching the fleet database; the error names the
    /// step that failed (Stado resolve, Skarbiec, or Postgres).
    pub fn open() -> Result<Self, String> {
        run_db(|_| Ok(()))?;
        Ok(Self)
    }
    /// Where memory is kept, as health reports and pickers show it.
    pub fn location(&self) -> &'static str {
        "fleet database jeden (memories, memory_*)"
    }

    pub fn embedding_health(
        &self,
        provider: Option<&dyn EmbeddingProvider>,
    ) -> Result<EmbeddingHealth, String> {
        embeddings::health(provider)
    }
    pub fn edges(&self, id: &str) -> Result<Vec<MemoryEdge>, String> {
        let id = id.to_owned();
        run_db(move |client| conflict::edges(client, &id))
    }
    pub fn rebuild_embeddings(&self, provider: &dyn EmbeddingProvider) -> Result<usize, String> {
        embeddings::rebuild(provider)
    }

    pub fn acquire_scope_lock(
        &self,
        scope: &MemoryScope,
        owner: &str,
        ttl_ms: i64,
    ) -> Result<bool, String> {
        let (scope, owner) = (scope.clone(), owner.to_owned());
        run_db(move |client| {
            let now = now_ms();
            let mut tx = client.transaction().map_err(sql)?;
            tx.execute(
                "DELETE FROM memory_scope_locks WHERE expires_at<$1",
                &[&now],
            )
            .map_err(sql)?;
            let acquired = tx
                .execute(
                    "INSERT INTO memory_scope_locks(scope_kind,scope_id,owner,expires_at) VALUES($1,$2,$3,$4)
                     ON CONFLICT(scope_kind,scope_id) DO UPDATE SET owner=excluded.owner,expires_at=excluded.expires_at
                     WHERE memory_scope_locks.owner=excluded.owner",
                    &[&scope.kind, &scope.id, &owner, &(now + ttl_ms.clamp(1_000, 300_000))],
                )
                .map_err(sql)?
                == 1;
            tx.commit().map_err(sql)?;
            Ok(acquired)
        })
    }
    pub fn release_scope_lock(&self, scope: &MemoryScope, owner: &str) -> Result<(), String> {
        let (scope, owner) = (scope.clone(), owner.to_owned());
        run_db(move |client| {
            client
                .execute(
                    "DELETE FROM memory_scope_locks WHERE scope_kind=$1 AND scope_id=$2 AND owner=$3",
                    &[&scope.kind, &scope.id, &owner],
                )
                .map_err(sql)?;
            Ok(())
        })
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
        let fingerprint = fingerprint.to_owned();
        let description = bounded_redacted(description, MAX_MEMORY_CHARS);
        let session_id = session_id.to_owned();
        run_db(move |client| {
            let mut tx = client.transaction().map_err(sql)?;
            let prior = tx
                .query_opt(
                    "SELECT sessions_json,occurrences,verified FROM memory_workflows WHERE fingerprint=$1 FOR UPDATE",
                    &[&fingerprint],
                )
                .map_err(sql)?
                .map(|row| (row.get::<_, String>(0), row.get::<_, i64>(1), row.get::<_, bool>(2)));
            let (mut sessions, occ, was_verified) = prior
                .map(|(j, o, v)| {
                    (
                        serde_json::from_str::<Vec<String>>(&j).unwrap_or_default(),
                        o,
                        v,
                    )
                })
                .unwrap_or((Vec::new(), 0, false));
            if !sessions.iter().any(|s| *s == session_id) {
                sessions.push(session_id)
            }
            let occurrences = occ + 1;
            let is_verified = was_verified || verified;
            let sessions = serde_json::to_string(&sessions).map_err(|e| e.to_string())?;
            tx.execute(
                "INSERT INTO memory_workflows(fingerprint,description,sessions_json,occurrences,verified,updated_at)
                 VALUES($1,$2,$3,$4,$5,$6) ON CONFLICT(fingerprint) DO UPDATE SET description=excluded.description,
                 sessions_json=excluded.sessions_json,occurrences=excluded.occurrences,verified=excluded.verified,
                 updated_at=excluded.updated_at",
                &[&fingerprint, &description, &sessions, &occurrences, &is_verified, &now_ms()],
            )
            .map_err(sql)?;
            let skill = if occurrences >= 3 && is_verified {
                let id = format!(
                    "skill_{}",
                    &hex::encode(Sha256::digest(fingerprint.as_bytes()))[..24]
                );
                let body = format!("# Managed workflow\n\n{description}");
                tx.execute(
                    "INSERT INTO memory_managed_skills(id,workflow_fingerprint,body,created_at)
                     VALUES($1,$2,$3,$4) ON CONFLICT DO NOTHING",
                    &[&id, &fingerprint, &body, &now_ms()],
                )
                .map_err(sql)?;
                Some(id)
            } else {
                None
            };
            tx.commit().map_err(sql)?;
            Ok(skill)
        })
    }
}
pub(super) fn load_record(
    client: &mut impl GenericClient,
    id: &str,
) -> Result<Option<MemoryRecord>, String> {
    Ok(client
        .query_opt(
            &*format!("SELECT {RECORD_COLUMNS} FROM memories WHERE id=$1"),
            &[&id],
        )
        .map_err(sql)?
        .map(|row| row_record(&row)))
}
pub(super) fn row_record(row: &postgres::Row) -> MemoryRecord {
    let tags: String = row.get(5);
    let source: String = row.get(6);
    MemoryRecord {
        id: row.get(0),
        kind: row.get(1),
        scope: MemoryScope {
            kind: row.get(2),
            id: row.get(3),
        },
        text: row.get(4),
        tags: serde_json::from_str(&tags).unwrap_or_default(),
        source: serde_json::from_str(&source).unwrap_or(MemorySource {
            origin: "unknown".into(),
            session_id: None,
            entry_id: None,
        }),
        confidence: row.get(7),
        status: row.get(8),
        created_at: row.get(9),
        updated_at: row.get(10),
        logical_key: row.get(11),
        revision: row.get(12),
        valid_from: row.get(13),
        valid_to: row.get(14),
        supersedes: row.get(15),
        tombstone: row.get(16),
    }
}
pub(super) fn logical_key(kind: &str, scope: &MemoryScope, text: &str) -> String {
    let mut hash = Sha256::new();
    for part in [kind, scope.kind.as_str(), scope.id.as_str(), text] {
        hash.update(part.as_bytes());
        hash.update([0])
    }
    format!("auto:{}", hex::encode(hash.finalize()))
}
