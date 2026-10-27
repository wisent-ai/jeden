use super::*;
use crate::fleet::{run_db, sql};
use crate::tally::ORIGIN;
use entities::memory;
use sea_orm::{ConnectionTrait, EntityTrait};
use sha2::{Digest, Sha256};
pub(super) mod entities;
mod maintenance;
mod recall;
mod writes;

/// A compaction summary is the model's condensation of a conversation, not a
/// belief anyone stated with a confidence, so it is kept at the bottom of the
/// confidence range the memory schema declares: every memory whose confidence
/// was stated ranks ahead of it.
const UNVOUCHED: f64 = ORIGIN as f64;

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

    /// Keeps a conversation's compaction summary as a summary memory of
    /// `scope`, unvouched (see [`UNVOUCHED`]).
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
        let tags = ["consolidated".to_string(), "model-assisted".to_string()];
        self.remember("summary", scope, summary, &tags, &source, UNVOUCHED)
    }
}
pub(super) async fn load_record(
    db: &impl ConnectionTrait,
    id: &str,
) -> Result<Option<MemoryRecord>, String> {
    memory::Entity::find_by_id(id.to_owned())
        .one(db)
        .await
        .map_err(sql)?
        .map(record)
        .transpose()
}
/// One stored memory. Its tags and source are written as JSON; a row whose
/// JSON does not parse is damaged and refused with the column named, not
/// read as untagged memory of "unknown" origin.
pub(super) fn record(row: memory::Model) -> Result<MemoryRecord, String> {
    let tags = serde_json::from_str(&row.tags_json).map_err(|error| {
        format!(
            "memory {} has tags_json that does not parse ({error}): {}",
            row.id, row.tags_json
        )
    })?;
    let source = serde_json::from_str(&row.source_json).map_err(|error| {
        format!(
            "memory {} has source_json that does not parse ({error}): {}",
            row.id, row.source_json
        )
    })?;
    Ok(MemoryRecord {
        tags,
        source,
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
    })
}
/// The key a memory is revised under: its kind, scope and text, each
/// followed by a NUL byte so no two different tuples hash alike.
pub(super) fn logical_key(kind: &str, scope: &MemoryScope, text: &str) -> String {
    let mut hash = Sha256::new();
    for part in [kind, scope.kind.as_str(), scope.id.as_str(), text] {
        hash.update(part.as_bytes());
        hash.update(b"\0")
    }
    format!("auto:{}", hex::encode(hash.finalize()))
}
