use super::*;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};

static NEXT_FIXTURE: AtomicU64 = AtomicU64::new(1);

struct Fixture {
    dir: PathBuf,
    db: PathBuf,
}

impl Fixture {
    fn new(name: &str) -> Self {
        let dir = std::env::temp_dir().join(format!(
            "jeden-semantic-memory-{name}-{}-{}",
            std::process::id(),
            NEXT_FIXTURE.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let db = dir.join("memory.sqlite3");
        Self { dir, db }
    }

    fn store(&self) -> MemoryStore {
        MemoryStore::open(&self.db).unwrap()
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

fn scope(id: &str) -> MemoryScope {
    MemoryScope {
        kind: "repo".into(),
        id: id.into(),
    }
}

fn source() -> MemorySource {
    MemorySource {
        origin: "semantic-memory-test".into(),
        session_id: Some("session-1".into()),
        entry_id: Some("entry-1".into()),
    }
}

#[test]
fn forgotten_memory_has_zero_recall_and_remains_forgotten_after_reopen() {
    let fixture = Fixture::new("forgotten-zero-recall");
    let store = fixture.store();
    let target_scope = scope("target");
    store
        .remember(
            "fact",
            &target_scope,
            "the deployment codename is heliotrope",
            &[],
            &source(),
            0.9,
        )
        .unwrap();

    assert_eq!(
        store
            .recall(&FtsBackend, &target_scope, "heliotrope", 10)
            .unwrap()
            .len(),
        1
    );
    assert_eq!(store.forget_scope(&target_scope).unwrap(), 1);
    assert!(store
        .recall(&FtsBackend, &target_scope, "heliotrope", 10)
        .unwrap()
        .is_empty());

    drop(store);
    let reopened = fixture.store();
    assert!(reopened
        .recall(&FtsBackend, &target_scope, "heliotrope", 10)
        .unwrap()
        .is_empty());
    let records = reopened.list(10).unwrap();
    assert_eq!(records.len(), 1);
    assert_eq!(records[0].status, "forgotten");
    assert_eq!(records[0].text, "the deployment codename is heliotrope");
}

#[test]
fn recall_is_scope_isolated_while_global_memory_is_shared() {
    let fixture = Fixture::new("scope-isolation");
    let store = fixture.store();
    let alpha = scope("alpha");
    let beta = scope("beta");
    let global = MemoryScope {
        kind: "global".into(),
        id: "global".into(),
    };

    store
        .remember(
            "fact",
            &alpha,
            "alpha owns the narwhal release",
            &[],
            &source(),
            0.9,
        )
        .unwrap();
    store
        .remember(
            "fact",
            &global,
            "global narwhal safety policy",
            &[],
            &source(),
            0.9,
        )
        .unwrap();

    let alpha_text = store
        .recall(&FtsBackend, &alpha, "narwhal", 10)
        .unwrap()
        .into_iter()
        .map(|hit| hit.record.text)
        .collect::<Vec<_>>();
    assert_eq!(alpha_text.len(), 2);
    assert!(alpha_text
        .iter()
        .any(|text| text == "alpha owns the narwhal release"));
    assert!(alpha_text
        .iter()
        .any(|text| text == "global narwhal safety policy"));

    let beta_text = store
        .recall(&FtsBackend, &beta, "narwhal", 10)
        .unwrap()
        .into_iter()
        .map(|hit| hit.record.text)
        .collect::<Vec<_>>();
    assert_eq!(beta_text, vec!["global narwhal safety policy"]);
}

#[test]
fn secrets_are_redacted_before_persistence_and_cannot_be_recalled() {
    let fixture = Fixture::new("redaction");
    let store = fixture.store();
    let target_scope = scope("redaction");
    let raw_secret = "super-secret-value";

    let record = store
        .remember(
            "fact",
            &target_scope,
            &format!("production token={raw_secret} belongs to the aurora service"),
            &[],
            &source(),
            0.9,
        )
        .unwrap();
    assert_eq!(
        record.text,
        "production [REDACTED] belongs to the aurora service"
    );

    drop(store);
    let reopened = fixture.store();
    let records = reopened.list(10).unwrap();
    assert_eq!(
        records[0].text,
        "production [REDACTED] belongs to the aurora service"
    );
    assert!(!records[0].text.contains(raw_secret));
    assert!(reopened
        .recall(&FtsBackend, &target_scope, raw_secret, 10)
        .unwrap()
        .is_empty());
    assert_eq!(
        reopened
            .recall(&FtsBackend, &target_scope, "aurora", 10)
            .unwrap()[0]
            .record
            .text,
        "production [REDACTED] belongs to the aurora service"
    );
}

struct KeywordEmbedding;
impl EmbeddingProvider for KeywordEmbedding {
    fn name(&self) -> &str {
        "keyword-test-v1"
    }
    fn embed(&self, texts: &[String]) -> Result<Vec<Vec<f32>>, String> {
        Ok(texts
            .iter()
            .map(|text| {
                vec![
                    text.matches("aurora").count() as f32,
                    text.matches("nebula").count() as f32,
                ]
            })
            .collect())
    }
}

#[test]
fn logical_revisions_deduplicate_supersede_and_support_time_travel() {
    let fixture = Fixture::new("logical-revisions");
    let store = fixture.store();
    let target = scope("revision");
    let base = now_ms() - 10_000;
    let first = store
        .remember_with_key(
            "fact",
            &target,
            "deploy.region",
            "aurora deploys in east",
            &[],
            &source(),
            0.8,
            Some(base),
        )
        .unwrap();
    let duplicate = store
        .remember_with_key(
            "fact",
            &target,
            "deploy.region",
            "aurora deploys in east",
            &[],
            &source(),
            0.8,
            Some(base + 1),
        )
        .unwrap();
    assert_eq!(first.id, duplicate.id);
    let second = store
        .remember_with_key(
            "fact",
            &target,
            "deploy.region",
            "aurora deploys in west",
            &[],
            &source(),
            0.9,
            Some(base + 5_000),
        )
        .unwrap();
    assert_eq!(second.revision, 2);
    assert_eq!(second.supersedes.as_deref(), Some(first.id.as_str()));
    assert_eq!(
        store.recall_at(&target, "east", 10, base + 1_000).unwrap()[0]
            .record
            .id,
        first.id
    );
    assert!(store
        .recall(&FtsBackend, &target, "east", 10)
        .unwrap()
        .is_empty());
    assert_eq!(
        store.recall(&FtsBackend, &target, "west", 10).unwrap()[0]
            .record
            .id,
        second.id
    );
    store
        .tombstone(&target, "deploy.region", &source(), None)
        .unwrap();
    assert!(store
        .recall(&FtsBackend, &target, "west", 10)
        .unwrap()
        .is_empty());
}

#[test]
fn unresolved_conflicts_are_returned_as_one_provenanced_group() {
    let fixture = Fixture::new("conflicts");
    let store = fixture.store();
    let target = scope("conflict");
    let east = store
        .remember_with_key(
            "fact",
            &target,
            "region.east",
            "aurora region is east",
            &[],
            &source(),
            0.8,
            None,
        )
        .unwrap();
    let west = store
        .remember_with_key(
            "fact",
            &target,
            "region.west",
            "aurora region is west",
            &[],
            &source(),
            0.8,
            None,
        )
        .unwrap();
    store
        .add_edge(&east.id, &west.id, MemoryRelation::Conflicts, &source())
        .unwrap();
    let hits = store
        .recall(&FtsBackend, &target, "aurora region", 10)
        .unwrap();
    assert_eq!(hits.len(), 2);
    assert!(hits.iter().all(|hit| hit.conflict_group.is_some()));
    assert_eq!(hits[0].conflict_group, hits[1].conflict_group);
    assert!(hits.iter().all(|hit| hit
        .provenance
        .edges
        .iter()
        .any(|edge| edge.relation == MemoryRelation::Conflicts)));
    assert_eq!(
        store
            .resolve_conflict(&east.id, std::slice::from_ref(&west.id), &source())
            .unwrap(),
        1
    );
    let resolved = store
        .recall(&FtsBackend, &target, "aurora region", 10)
        .unwrap();
    assert_eq!(resolved.len(), 1);
    assert_eq!(resolved[0].record.id, east.id);
}

#[test]
fn rebuildable_embeddings_and_lexical_only_health_never_fake_semantics() {
    let fixture = Fixture::new("embedding-rebuild");
    let store = fixture.store();
    let target = scope("embedding");
    store
        .remember(
            "fact",
            &target,
            "aurora launch checklist",
            &[],
            &source(),
            0.9,
        )
        .unwrap();
    let health = store.health().unwrap();
    assert_eq!(health["retrievalMode"], "lexical-only");
    assert_eq!(health["embeddingAvailable"], false);
    assert_eq!(store.rebuild_embeddings(&KeywordEmbedding).unwrap(), 1);
    assert_eq!(store.rebuild_embeddings(&KeywordEmbedding).unwrap(), 1);
    let backend = HybridBackend::new(Some(&KeywordEmbedding));
    let hit = store
        .recall(&backend, &target, "aurora", 1)
        .unwrap()
        .remove(0);
    assert!(hit.components.semantic > 0.0);
    assert!(hit.components.lexical > 0.0);
    assert_eq!(hit.provenance.revision, 1);
}

#[test]
fn temporal_half_life_is_reported_and_affects_hybrid_ordering() {
    let fixture = Fixture::new("half-life");
    let store = fixture.store();
    let target = scope("half-life");
    let old = store
        .remember_with_key(
            "fact",
            &target,
            "old",
            "aurora shared wording",
            &[],
            &source(),
            0.8,
            None,
        )
        .unwrap();
    let fresh = store
        .remember_with_key(
            "fact",
            &target,
            "fresh",
            "aurora shared wording",
            &[],
            &source(),
            0.8,
            None,
        )
        .unwrap();
    store
        .connect()
        .unwrap()
        .execute(
            "UPDATE memories SET updated_at=?2 WHERE id=?1",
            rusqlite::params![old.id, now_ms() - 10_000],
        )
        .unwrap();
    let backend = HybridBackend {
        provider: None,
        as_of: Some(now_ms()),
        half_life_ms: 1_000.0,
    };
    let hits = store.recall(&backend, &target, "aurora", 10).unwrap();
    assert_eq!(hits[0].record.id, fresh.id);
    assert!(hits[0].components.temporal > hits[1].components.temporal);
}

#[test]
fn schema_fts_provenance_and_evaluation_hooks_are_operational() {
    let fixture = Fixture::new("schema-hooks");
    let store = fixture.store();
    let target = scope("hooks");
    let record = store
        .remember(
            "fact",
            &target,
            "quasar operational guide",
            &[],
            &source(),
            0.9,
        )
        .unwrap();
    let conn = store.connect().unwrap();
    let version: i64 = conn
        .query_row("PRAGMA user_version", [], |row| row.get(0))
        .unwrap();
    assert_eq!(version, schema::SCHEMA_VERSION);
    let migration: String = conn
        .query_row(
            "SELECT migration_id FROM migration_history WHERE version=3",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(migration, "memory-003-revision-semantic");
    drop(conn);
    let hit = store
        .recall(&FtsBackend, &target, "quasar", 1)
        .unwrap()
        .remove(0);
    assert_eq!(hit.provenance.memory_id, record.id);
    let ranked = vec!["x".to_string(), record.id.clone()];
    let relevant = vec![record.id.clone()];
    assert_eq!(recall_at_k(&ranked, &relevant, 2), 1.0);
    assert_eq!(mean_reciprocal_rank(&ranked, &relevant), 0.5);
    let relevance = std::collections::HashMap::from([(record.id.clone(), 2.0)]);
    assert!(ndcg_at_k(&ranked, &relevance, 2) > 0.0);
}
