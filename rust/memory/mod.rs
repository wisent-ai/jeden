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
pub use ranking::{FtsBackend, HybridBackend, RankedCandidate, ScoreComponents, SemanticBackend};
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

/// The scope a memory call reads and writes, from its `scope` argument. A call
/// that names none, or names the repository, is about the repository at
/// `cwd`; an object names a kind and an id, either of which may be left to
/// that same reading; any other shape is refused naming it.
pub fn scope_from_value(value: Option<&Value>, cwd: &Path) -> Result<MemoryScope, String> {
    let repository = |id: String| MemoryScope {
        kind: REPO_SCOPE.into(),
        id,
    };
    match value {
        None | Some(Value::Null) => Ok(repository(cwd.display().to_string())),
        Some(Value::String(kind)) if kind == REPO_SCOPE => {
            Ok(repository(cwd.display().to_string()))
        }
        Some(Value::String(kind)) => Ok(MemoryScope {
            kind: kind.clone(),
            id: kind.clone(),
        }),
        Some(Value::Object(fields)) => {
            let kind = fields.get("kind").and_then(Value::as_str);
            let id = fields.get("id").and_then(Value::as_str);
            Ok(match (kind, id) {
                (None, None) => repository(cwd.display().to_string()),
                (None, Some(id)) => repository(id.to_string()),
                (Some(kind), None) if kind == REPO_SCOPE => repository(cwd.display().to_string()),
                (Some(kind), None) => MemoryScope {
                    kind: kind.to_string(),
                    id: kind.to_string(),
                },
                (Some(kind), Some(id)) => MemoryScope {
                    kind: kind.to_string(),
                    id: id.to_string(),
                },
            })
        }
        Some(other) => Err(format!(
            "memory scope must be a kind name or an object with kind and id, not {other}"
        )),
    }
}

/// The scope kind of the repository a call runs in.
const REPO_SCOPE: &str = "repo";

/// A new identifier under `prefix`: a random UUID, so ids minted anywhere do
/// not collide.
pub(super) fn stable_id(prefix: &str) -> String {
    format!("{prefix}_{}", uuid::Uuid::new_v4().simple())
}
/// Milliseconds since the Unix epoch as the store's column type, or why the
/// clock has none to give.
pub(super) fn now_ms() -> Result<i64, String> {
    crate::clock::unix_millis_i64().map_err(|error| error.to_string())
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
        // A pattern that does not compile fails closed: skipped, the
        // credential it recognises was kept in memory unredacted.
        match regex::Regex::new(pattern) {
            Ok(re) => text = re.replace_all(&text, "[REDACTED]").into_owned(),
            Err(error) => {
                return format!(
                    "[REDACTED: the credential pattern {pattern} does not compile: {error}]"
                )
            }
        }
    }
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}
