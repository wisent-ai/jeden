use super::{EmbeddingProvider, MemoryScope};
use rusqlite::{params, Connection};
use serde::Serialize;
use std::collections::HashMap;

const DEFAULT_HALF_LIFE_MS: f64 = 30.0 * 24.0 * 60.0 * 60.0 * 1_000.0;

#[derive(Debug, Clone, Copy, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ScoreComponents {
    pub lexical: f64,
    pub semantic: f64,
    pub confidence: f64,
    pub temporal: f64,
}

impl Default for ScoreComponents {
    fn default() -> Self {
        Self {
            lexical: 0.0,
            semantic: 0.0,
            confidence: 0.0,
            temporal: 0.0,
        }
    }
}

#[derive(Debug, Clone)]
pub struct RankedCandidate {
    pub id: String,
    pub score: f64,
    pub components: ScoreComponents,
}

pub trait SemanticBackend {
    fn name(&self) -> &'static str;
    fn recall(
        &self,
        conn: &Connection,
        scope: &MemoryScope,
        query: &str,
        limit: usize,
    ) -> Result<Vec<RankedCandidate>, String>;
}

pub struct FtsBackend;

impl SemanticBackend for FtsBackend {
    fn name(&self) -> &'static str {
        "sqlite-fts5"
    }
    fn recall(
        &self,
        conn: &Connection,
        scope: &MemoryScope,
        query: &str,
        limit: usize,
    ) -> Result<Vec<RankedCandidate>, String> {
        rank(
            conn,
            scope,
            query,
            limit,
            super::now_ms(),
            DEFAULT_HALF_LIFE_MS,
            None,
        )
    }
}

pub struct HybridBackend<'a> {
    pub provider: Option<&'a dyn EmbeddingProvider>,
    pub as_of: Option<i64>,
    pub half_life_ms: f64,
}

impl<'a> HybridBackend<'a> {
    pub fn new(provider: Option<&'a dyn EmbeddingProvider>) -> Self {
        Self {
            provider,
            as_of: None,
            half_life_ms: DEFAULT_HALF_LIFE_MS,
        }
    }
}

impl SemanticBackend for HybridBackend<'_> {
    fn name(&self) -> &'static str {
        if self.provider.is_some() {
            "hybrid-fts-semantic"
        } else {
            "sqlite-fts5"
        }
    }
    fn recall(
        &self,
        conn: &Connection,
        scope: &MemoryScope,
        query: &str,
        limit: usize,
    ) -> Result<Vec<RankedCandidate>, String> {
        let semantic = match self.provider.filter(|p| p.available()) {
            Some(provider) if !query.trim().is_empty() => {
                let vectors = provider.embed(&[query.to_string()])?;
                let vector = vectors
                    .first()
                    .ok_or("embedding provider returned no query vector")?;
                Some(super::embeddings::semantic_scores(conn, vector)?)
            }
            _ => None,
        };
        rank(
            conn,
            scope,
            query,
            limit,
            self.as_of.unwrap_or_else(super::now_ms),
            self.half_life_ms,
            semantic.as_deref(),
        )
    }
}

fn rank(
    conn: &Connection,
    scope: &MemoryScope,
    query: &str,
    limit: usize,
    as_of: i64,
    half_life_ms: f64,
    semantic: Option<&[(String, f64)]>,
) -> Result<Vec<RankedCandidate>, String> {
    let safe_query = query
        .split_whitespace()
        .filter(|word| !word.is_empty())
        .map(|word| format!("\"{}\"*", word.replace('"', "\"\"")))
        .collect::<Vec<_>>()
        .join(" OR ");
    let mut lexical = HashMap::new();
    if safe_query.is_empty() {
        let mut stmt = conn.prepare(
            "SELECT id FROM memories WHERE status!='forgotten' AND tombstone=0 AND valid_from<=?3 AND (valid_to IS NULL OR valid_to>?3) AND ((scope_kind=?1 AND scope_id=?2) OR scope_kind='global')"
        ).map_err(|e| e.to_string())?;
        for id in stmt
            .query_map(params![scope.kind, scope.id, as_of], |row| {
                row.get::<_, String>(0)
            })
            .map_err(|e| e.to_string())?
        {
            lexical.insert(id.map_err(|e| e.to_string())?, 1.0);
        }
    } else {
        let mut stmt = conn.prepare(
            "SELECT m.id,bm25(memories_fts) FROM memories_fts JOIN memories m ON m.rowid=memories_fts.rowid
             WHERE memories_fts MATCH ?1 AND m.status!='forgotten' AND m.tombstone=0
             AND m.valid_from<=?4 AND (m.valid_to IS NULL OR m.valid_to>?4)
             AND ((m.scope_kind=?2 AND m.scope_id=?3) OR m.scope_kind='global')"
        ).map_err(|e| e.to_string())?;
        let rows = stmt
            .query_map(params![safe_query, scope.kind, scope.id, as_of], |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, f64>(1)?))
            })
            .map_err(|e| e.to_string())?;
        for row in rows {
            let (id, bm25) = row.map_err(|e| e.to_string())?;
            lexical.insert(id, 1.0 / (1.0 + bm25.abs()));
        }
    }
    let semantic = semantic
        .unwrap_or(&[])
        .iter()
        .cloned()
        .collect::<HashMap<_, _>>();
    let ids = lexical
        .keys()
        .chain(semantic.keys())
        .cloned()
        .collect::<std::collections::HashSet<_>>();
    let mut ranked = Vec::new();
    for id in ids {
        let row = conn.query_row(
            "SELECT confidence,updated_at FROM memories WHERE id=?1 AND status!='forgotten' AND tombstone=0 AND valid_from<=?2 AND (valid_to IS NULL OR valid_to>?2) AND ((scope_kind=?3 AND scope_id=?4) OR scope_kind='global')",
            params![id, as_of, scope.kind, scope.id], |row| Ok((row.get::<_, f64>(0)?, row.get::<_, i64>(1)?)),
        ).ok();
        let Some((confidence, updated_at)) = row else {
            continue;
        };
        let age = as_of.saturating_sub(updated_at).max(0) as f64;
        let temporal = if half_life_ms > 0.0 {
            2.0_f64.powf(-age / half_life_ms)
        } else {
            1.0
        };
        let components = ScoreComponents {
            lexical: lexical.get(&id).copied().unwrap_or(0.0),
            semantic: semantic.get(&id).copied().unwrap_or(0.0).max(0.0),
            confidence: confidence.clamp(0.0, 1.0),
            temporal,
        };
        let score = if semantic.is_empty() {
            0.65 * components.lexical + 0.20 * components.confidence + 0.15 * components.temporal
        } else {
            0.40 * components.lexical
                + 0.35 * components.semantic
                + 0.15 * components.confidence
                + 0.10 * components.temporal
        };
        ranked.push(RankedCandidate {
            id,
            score,
            components,
        });
    }
    ranked.sort_by(|a, b| b.score.total_cmp(&a.score).then_with(|| a.id.cmp(&b.id)));
    ranked.truncate(limit.min(100));
    Ok(ranked)
}

pub fn recall_at_k(ranked: &[String], relevant: &[String], k: usize) -> f64 {
    if relevant.is_empty() {
        return 0.0;
    }
    ranked
        .iter()
        .take(k)
        .filter(|id| relevant.contains(id))
        .count() as f64
        / relevant.len() as f64
}

pub fn mean_reciprocal_rank(ranked: &[String], relevant: &[String]) -> f64 {
    ranked
        .iter()
        .position(|id| relevant.contains(id))
        .map(|index| 1.0 / (index + 1) as f64)
        .unwrap_or(0.0)
}

pub fn ndcg_at_k(ranked: &[String], relevance: &HashMap<String, f64>, k: usize) -> f64 {
    fn dcg(values: impl Iterator<Item = f64>) -> f64 {
        values
            .enumerate()
            .map(|(i, rel)| (2.0_f64.powf(rel) - 1.0) / ((i + 2) as f64).log2())
            .sum()
    }
    let actual = dcg(ranked
        .iter()
        .take(k)
        .map(|id| relevance.get(id).copied().unwrap_or(0.0)));
    let mut ideal = relevance.values().copied().collect::<Vec<_>>();
    ideal.sort_by(|a, b| b.total_cmp(a));
    let best = dcg(ideal.into_iter().take(k));
    if best == 0.0 {
        0.0
    } else {
        actual / best
    }
}
