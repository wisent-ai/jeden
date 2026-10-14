mod conflict;
mod outbox;
mod retrieval;
mod schema;
mod store;
mod worker;

use retrieval::{embeddings, ranking};

use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::path::Path;

pub use embeddings::{EmbeddingHealth, EmbeddingProvider};
pub use outbox::{OutboxConsumer, OutboxEvent};
pub use ranking::{
    mean_reciprocal_rank, ndcg_at_k, recall_at_k, FtsBackend, HybridBackend, RankedCandidate,
    ScoreComponents, SemanticBackend,
};
pub(crate) use schema::SCHEMA;
pub use store::MemoryStore;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Hash)]
#[serde(rename_all = "camelCase")]
pub struct MemoryScope {
    pub kind: String,
    pub id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct MemorySource {
    pub origin: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub session_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub entry_id: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MemoryRecord {
    pub id: String,
    pub kind: String,
    pub scope: MemoryScope,
    pub logical_key: String,
    pub revision: i64,
    pub text: String,
    pub tags: Vec<String>,
    pub source: MemorySource,
    pub confidence: f64,
    pub status: String,
    pub valid_from: i64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub valid_to: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub supersedes: Option<String>,
    pub tombstone: bool,
    pub created_at: i64,
    pub updated_at: i64,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum MemoryRelation {
    Supports,
    Conflicts,
    Duplicates,
}

impl MemoryRelation {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Supports => "supports",
            Self::Conflicts => "conflicts",
            Self::Duplicates => "duplicates",
        }
    }
    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "supports" => Some(Self::Supports),
            "conflicts" => Some(Self::Conflicts),
            "duplicates" => Some(Self::Duplicates),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MemoryEdge {
    pub from_id: String,
    pub to_id: String,
    pub relation: MemoryRelation,
    pub created_at: i64,
    pub provenance: MemorySource,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RecallHit {
    pub record: MemoryRecord,
    pub score: f64,
    pub components: ScoreComponents,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub conflict_group: Option<String>,
    pub provenance: RecallProvenance,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RecallProvenance {
    pub backend: String,
    pub query: String,
    pub source: MemorySource,
    pub memory_id: String,
    pub logical_key: String,
    pub revision: i64,
    pub edges: Vec<MemoryEdge>,
}

pub trait Consolidator {
    fn consolidate(&self, candidates: &[MemoryRecord], max_chars: usize) -> Result<String, String>;
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LeasedJob {
    pub id: String,
    pub kind: String,
    pub payload: Value,
    pub attempts: i64,
    pub lease_until: i64,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MemoryQueueJob {
    pub id: String,
    pub kind: String,
    pub state: String,
    pub attempts: i64,
    pub available_at: i64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub lease_owner: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub lease_until: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_error: Option<String>,
    pub created_at: i64,
    pub updated_at: i64,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MemoryQueueStatus {
    pub total: i64,
    pub pending: i64,
    pub queued: i64,
    pub leased: i64,
    pub done: i64,
    pub failed: i64,
    pub jobs: Vec<MemoryQueueJob>,
}

pub fn scope_from_value(value: Option<&Value>, cwd: &Path) -> MemoryScope {
    match value {
        Some(Value::Object(m)) => {
            let kind = m
                .get("kind")
                .and_then(Value::as_str)
                .unwrap_or("repo")
                .to_string();
            let id = m
                .get("id")
                .and_then(Value::as_str)
                .map(str::to_string)
                .unwrap_or_else(|| {
                    if kind == "repo" {
                        cwd.display().to_string()
                    } else {
                        kind.clone()
                    }
                });
            MemoryScope { kind, id }
        }
        Some(Value::String(s)) if s != "repo" => MemoryScope {
            kind: s.clone(),
            id: s.clone(),
        },
        _ => MemoryScope {
            kind: "repo".into(),
            id: cwd.display().to_string(),
        },
    }
}

pub(super) fn stable_id(prefix: &str) -> String {
    use rand::RngCore;
    let mut bytes = [0u8; 16];
    rand::thread_rng().fill_bytes(&mut bytes);
    format!("{prefix}_{}", hex::encode(bytes))
}
/// Milliseconds since the Unix epoch as the store's column type; a clock
/// before the epoch, or a count past an i64, is refused, never stored as zero
/// or clamped.
pub(super) fn now_ms() -> Result<i64, String> {
    let millis = crate::clock::unix_millis().map_err(|error| error.to_string())?;
    i64::try_from(millis).map_err(|error| {
        format!("the system clock reads {millis} ms since the Unix epoch, more than the memory store's i64 holds: {error}")
    })
}

/// The durable job that rebuilds the full-text index: what `/memory enqueue`
/// writes and the worker runs.
pub(crate) const REINDEX_JOB: &str = "reindex";
/// `value` with credentials redacted and whitespace runs joined, whole.
///
/// A credential is recognised by the format its issuer documents, never by
/// the word in front of it: a PEM private key block (RFC 7468 labels end in
/// `PRIVATE KEY`), a GitHub token (`gh` + token-kind letter + `_`, GitHub's
/// token format), a Stripe key (`sk_`/`pk_`/`rk_` + mode + `_` + body,
/// Stripe's API key format). The rule that redacted whatever followed
/// `password=`, `token:`, `api_key=` and three more spellings is gone: it
/// missed every other spelling and hid values that were no secret.
pub(crate) fn redacted(value: &str) -> String {
    let mut text = value.to_string();
    for pattern in [
        r"-----BEGIN [A-Z ]*PRIVATE KEY-----[\s\S]*?-----END [A-Z ]*PRIVATE KEY-----",
        r"\bgh[pousr]_[A-Za-z0-9_]+\b",
        r"\b[spr]k_[a-z]+_[A-Za-z0-9]+\b",
    ] {
        if let Ok(re) = regex::Regex::new(pattern) {
            text = re.replace_all(&text, "[REDACTED]").into_owned()
        }
    }
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}
