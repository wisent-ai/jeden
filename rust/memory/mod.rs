mod conflict;
mod embeddings;
mod ranking;
mod schema;
mod store;
mod worker;

use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::path::Path;

pub use embeddings::{EmbeddingHealth, EmbeddingProvider};
pub use ranking::{
    mean_reciprocal_rank, ndcg_at_k, recall_at_k, FtsBackend, HybridBackend, RankedCandidate,
    ScoreComponents, SemanticBackend,
};
pub use store::MemoryStore;
pub use worker::MAX_ATTEMPTS;
pub use worker::{OutboxConsumer, OutboxEvent};

pub const MAX_MEMORY_CHARS: usize = 2_000;
pub const MAX_CONTEXT_CHARS: usize = 12_000;

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
pub(super) fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
        .min(i64::MAX as u128) as i64
}
pub(crate) fn bounded_redacted(value: &str, max: usize) -> String {
    let mut text = value.to_string();
    for pattern in [
        r"\b(?:sk|pk|rk)_[A-Za-z0-9_\-]{12,}\b",
        r"\bgh[pousr]_[A-Za-z0-9_]{16,}\b",
        r"(?i)\b(?:password|passwd|token|secret|api[_-]?key)\s*[:=]\s*[^\s,;]+",
        r"-----BEGIN [A-Z ]*PRIVATE KEY-----[\s\S]*?-----END [A-Z ]*PRIVATE KEY-----",
    ] {
        if let Ok(re) = regex::Regex::new(pattern) {
            text = re.replace_all(&text, "[REDACTED]").into_owned()
        }
    }
    let normalized = text.split_whitespace().collect::<Vec<_>>().join(" ");
    if normalized.chars().count() > max {
        let mut out = normalized
            .chars()
            .take(max.saturating_sub(1))
            .collect::<String>();
        out.push('…');
        out
    } else {
        normalized
    }
}

#[cfg(test)]
mod tests;
