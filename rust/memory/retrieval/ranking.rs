use crate::fleet::{run_db, sql};
use crate::memory::store::entities::memory;
use crate::memory::{EmbeddingProvider, MemoryScope};
use sea_orm::sea_query::{Condition, Expr};
use sea_orm::{ColumnTrait, EntityTrait, QueryFilter, QuerySelect};
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
        scope: &MemoryScope,
        query: &str,
        limit: usize,
    ) -> Result<Vec<RankedCandidate>, String>;
}

pub struct FtsBackend;

impl SemanticBackend for FtsBackend {
    fn name(&self) -> &'static str {
        "postgres-fts"
    }
    fn recall(
        &self,
        scope: &MemoryScope,
        query: &str,
        limit: usize,
    ) -> Result<Vec<RankedCandidate>, String> {
        rank(
            scope,
            query,
            limit,
            crate::memory::now_ms(),
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
            "postgres-fts"
        }
    }
    fn recall(
        &self,
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
                Some(super::embeddings::semantic_scores(vector)?)
            }
            _ => None,
        };
        rank(
            scope,
            query,
            limit,
            self.as_of.unwrap_or_else(crate::memory::now_ms),
            self.half_life_ms,
            semantic.as_deref(),
        )
    }
}

/// The words of `query` as a prefix-matching `to_tsquery` expression; only
/// letters and digits survive, so no query text can change its syntax.
fn tsquery(query: &str) -> String {
    query
        .split(|c: char| !c.is_alphanumeric())
        .filter(|word| !word.is_empty())
        .map(|word| format!("{}:*", word.to_lowercase()))
        .collect::<Vec<_>>()
        .join(" | ")
}

/// Memories visible in `scope` at `as_of`: not forgotten, not a tombstone,
/// valid at that moment, in the scope or global.
fn visible(scope: &MemoryScope, as_of: i64) -> Condition {
    Condition::all()
        .add(memory::Column::Status.ne("forgotten"))
        .add(memory::Column::Tombstone.eq(false))
        .add(memory::Column::ValidFrom.lte(as_of))
        .add(
            Condition::any()
                .add(memory::Column::ValidTo.is_null())
                .add(memory::Column::ValidTo.gt(as_of)),
        )
        .add(
            Condition::any()
                .add(
                    Condition::all()
                        .add(memory::Column::ScopeKind.eq(scope.kind.clone()))
                        .add(memory::Column::ScopeId.eq(scope.id.clone())),
                )
                .add(memory::Column::ScopeKind.eq("global")),
        )
}

fn rank(
    scope: &MemoryScope,
    query: &str,
    limit: usize,
    as_of: i64,
    half_life_ms: f64,
    semantic: Option<&[(String, f64)]>,
) -> Result<Vec<RankedCandidate>, String> {
    let semantic = semantic
        .unwrap_or(&[])
        .iter()
        .cloned()
        .collect::<HashMap<_, _>>();
    let (scope, terms) = (scope.clone(), tsquery(query));
    let semantic_ids = semantic.keys().cloned().collect::<Vec<_>>();
    // id -> (lexical, confidence, updated_at)
    let rows: HashMap<String, (f64, f64, i64)> = run_db(move |db| async move {
        let lexical_score = if terms.is_empty() {
            Expr::cust("1.0::float8")
        } else {
            Expr::cust_with_values(
                "ts_rank(search, to_tsquery('simple', $1), 32)::float8",
                [terms.clone()],
            )
        };
        let mut lexical = memory::Entity::find()
            .select_only()
            .column(memory::Column::Id)
            .column_as(lexical_score, "lexical")
            .column(memory::Column::Confidence)
            .column(memory::Column::UpdatedAt)
            .filter(visible(&scope, as_of));
        if !terms.is_empty() {
            lexical = lexical.filter(Expr::cust_with_values(
                "search @@ to_tsquery('simple', $1)",
                [terms],
            ));
        }
        let mut rows = HashMap::new();
        for (id, score, confidence, updated_at) in lexical
            .into_tuple::<(String, f64, f64, i64)>()
            .all(&db)
            .await
            .map_err(sql)?
        {
            rows.insert(id, (score, confidence, updated_at));
        }
        if !semantic_ids.is_empty() {
            for (id, confidence, updated_at) in memory::Entity::find()
                .select_only()
                .column(memory::Column::Id)
                .column(memory::Column::Confidence)
                .column(memory::Column::UpdatedAt)
                .filter(memory::Column::Id.is_in(semantic_ids))
                .filter(visible(&scope, as_of))
                .into_tuple::<(String, f64, i64)>()
                .all(&db)
                .await
                .map_err(sql)?
            {
                rows.entry(id).or_insert((0.0, confidence, updated_at));
            }
        }
        Ok(rows)
    })?;
    let mut ranked = Vec::new();
    for (id, (lexical, confidence, updated_at)) in rows {
        let age = as_of.saturating_sub(updated_at).max(0) as f64;
        let temporal = if half_life_ms > 0.0 {
            2.0_f64.powf(-age / half_life_ms)
        } else {
            1.0
        };
        let components = ScoreComponents {
            lexical,
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
