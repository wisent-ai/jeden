use super::*;
use crate::fleet::{run_db, sql};
use entities::{managed_skill, memory, workflow};
use sea_orm::sea_query::OnConflict;
use sea_orm::{
    ActiveValue::Set, ColumnTrait, ConnectionTrait, EntityTrait, QueryFilter, QuerySelect,
    TransactionTrait,
};
use sha2::{Digest, Sha256};
pub(super) mod entities;
mod maintenance;
mod recall;
mod writes;

/// Jeden's memory, kept in the fleet database `jeden` so every host and
/// session of the operator reads and writes the same memories.
pub struct MemoryStore;

impl MemoryStore {
    /// Opens memory by reaching the fleet database; the error names the
    /// step that failed (Stado resolve, Skarbiec, or Postgres).
    pub fn open() -> Result<Self, String> {
        run_db(|_| async { Ok(()) })?;
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
        run_db(move |db| async move { conflict::edges(&db, &id).await })
    }
    pub fn rebuild_embeddings(&self, provider: &dyn EmbeddingProvider) -> Result<usize, String> {
        embeddings::rebuild(provider)
    }

    /// Every visible memory of `scope` ranked for `query`, one line each, up
    /// to `max_chars` characters when the caller states a budget and whole
    /// when it states none.
    pub fn pre_compaction_context(
        &self,
        scope: &MemoryScope,
        query: &str,
        max_chars: Option<usize>,
    ) -> Result<String, String> {
        let hits = self.recall(&FtsBackend, scope, query, usize::MAX)?;
        let mut out = String::new();
        for hit in hits {
            let line = format!(
                "[{}; {}; {}] {}\n",
                hit.record.id, hit.provenance.backend, hit.record.source.origin, hit.record.text
            );
            if let Some(cap) = max_chars {
                if out.chars().count() + line.chars().count() > cap {
                    break;
                }
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
            .recall(&FtsBackend, scope, "", usize::MAX)?
            .into_iter()
            .map(|h| h.record)
            .collect::<Vec<_>>();
        if candidates.len() < 2 {
            return Err("consolidation requires at least two memories".into());
        }
        let text = redacted(&model.consolidate(&candidates, max_chars)?);
        let source = MemorySource {
            origin: "model_consolidation".into(),
            session_id: None,
            entry_id: None,
        };
        self.remember(
            "summary",
            scope,
            &text,
            &["consolidated".into()],
            &source,
            0.7,
        )
    }
    pub fn persist_model_consolidation(
        &self,
        scope: &MemoryScope,
        summary: &str,
    ) -> Result<MemoryRecord, String> {
        let source = MemorySource {
            origin: "model_compaction".into(),
            session_id: None,
            entry_id: None,
        };
        let tags: [String; 2] = ["consolidated".into(), "model-assisted".into()];
        self.remember("summary", scope, summary, &tags, &source, 0.85)
    }

    pub fn record_workflow(
        &self,
        fingerprint: &str,
        description: &str,
        session_id: &str,
        verified: bool,
    ) -> Result<Option<String>, String> {
        let fingerprint = fingerprint.to_owned();
        let description = redacted(description);
        let session_id = session_id.to_owned();
        run_db(move |db| async move {
            let tx = db.begin().await.map_err(sql)?;
            let prior = workflow::Entity::find_by_id(fingerprint.clone())
                .lock_exclusive()
                .one(&tx)
                .await
                .map_err(sql)?;
            let (mut sessions, occurrences, was_verified) = prior
                .map(|row| {
                    let sessions = serde_json::from_str::<Vec<String>>(&row.sessions_json);
                    (sessions.unwrap_or_default(), row.occurrences, row.verified)
                })
                .unwrap_or((Vec::new(), 0, false));
            if !sessions.iter().any(|s| *s == session_id) {
                sessions.push(session_id)
            }
            let occurrences = occurrences + 1;
            let verified = was_verified || verified;
            workflow::Entity::insert(workflow::ActiveModel {
                fingerprint: Set(fingerprint.clone()),
                description: Set(description.clone()),
                sessions_json: Set(serde_json::to_string(&sessions).map_err(|e| e.to_string())?),
                occurrences: Set(occurrences),
                verified: Set(verified),
                updated_at: Set(now_ms()),
            })
            .on_conflict(
                OnConflict::column(workflow::Column::Fingerprint)
                    .update_columns([
                        workflow::Column::Description,
                        workflow::Column::SessionsJson,
                        workflow::Column::Occurrences,
                        workflow::Column::Verified,
                        workflow::Column::UpdatedAt,
                    ])
                    .to_owned(),
            )
            .exec_without_returning(&tx)
            .await
            .map_err(sql)?;
            let skill = if occurrences >= 3 && verified {
                let id = format!(
                    "skill_{}",
                    &hex::encode(Sha256::digest(fingerprint.as_bytes()))[..24]
                );
                managed_skill::Entity::insert(managed_skill::ActiveModel {
                    id: Set(id.clone()),
                    workflow_fingerprint: Set(fingerprint),
                    body: Set(format!("# Managed workflow\n\n{description}")),
                    created_at: Set(now_ms()),
                })
                .on_conflict(
                    OnConflict::column(managed_skill::Column::Id)
                        .do_nothing()
                        .to_owned(),
                )
                .exec_without_returning(&tx)
                .await
                .map_err(sql)?;
                Some(id)
            } else {
                None
            };
            tx.commit().await.map_err(sql)?;
            Ok(skill)
        })
    }
}
pub(super) async fn load_record(
    db: &impl ConnectionTrait,
    id: &str,
) -> Result<Option<MemoryRecord>, String> {
    Ok(memory::Entity::find_by_id(id.to_owned())
        .one(db)
        .await
        .map_err(sql)?
        .map(record))
}
pub(super) fn record(row: memory::Model) -> MemoryRecord {
    MemoryRecord {
        tags: serde_json::from_str(&row.tags_json).unwrap_or_default(),
        source: serde_json::from_str(&row.source_json).unwrap_or(MemorySource {
            origin: "unknown".into(),
            session_id: None,
            entry_id: None,
        }),
        scope: MemoryScope {
            kind: row.scope_kind,
            id: row.scope_id,
        },
        id: row.id,
        kind: row.kind,
        text: row.text,
        confidence: row.confidence,
        status: row.status,
        created_at: row.created_at,
        updated_at: row.updated_at,
        logical_key: row.logical_key,
        revision: row.revision,
        valid_from: row.valid_from,
        valid_to: row.valid_to,
        supersedes: row.supersedes,
        tombstone: row.tombstone,
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
