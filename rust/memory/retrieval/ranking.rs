use crate::fleet::{run_db, sql};
use crate::memory::store::entities::memory;
use crate::memory::{EmbeddingProvider, MemoryScope};
use sea_orm::sea_query::{Condition, Expr};
use sea_orm::{ColumnTrait, EntityTrait, QueryFilter, QuerySelect};
use serde::Serialize;
use std::collections::HashMap;

/// Reciprocal rank fusion's k, as its authors fixed it: Cormack, Clarke and
/// Büttcher, "Reciprocal Rank Fusion outperforms Condorcet and individual Rank
/// Learning Methods", SIGIR 2009, <https://dl.acm.org/doi/10.1145/1571941.1572114>.
const RRF_K: f64 = 60.0;
/// The rank that paper gives the first item of an ordering: r(d) counts from
/// one, <https://dl.acm.org/doi/10.1145/1571941.1572114>.
const RRF_FIRST_RANK: f64 = 1.0;

/// Where a recalled memory stands in each ordering the recall fuses, first at
/// 0. No ordering is weighted against another, so none of these is a score.
#[derive(Debug, Clone, Copy, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ScoreComponents {
    /// Among the memories matching the query's words, by full-text rank;
    /// absent when the query has no words or the memory came by meaning only.
    pub lexical_position: Option<usize>,
    /// Among the memories with an embedding, by closeness to the query's;
    /// absent when no embedding was asked or this memory has none.
    pub semantic_position: Option<usize>,
    /// By the confidence the memory was written with, highest first.
    pub confidence_position: usize,
    /// By when the memory was last updated, newest first.
    pub recency_position: usize,
}

#[derive(Debug, Clone)]
pub struct RankedCandidate {
    pub id: String,
    /// The reciprocal rank fusion of the candidate's positions.
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
        rank(scope, query, limit, crate::memory::now_ms()?, None)
    }
}

pub struct HybridBackend<'a> {
    pub provider: Option<&'a dyn EmbeddingProvider>,
    pub as_of: Option<i64>,
}

impl<'a> HybridBackend<'a> {
    pub fn new(provider: Option<&'a dyn EmbeddingProvider>) -> Self {
        Self {
            provider,
            as_of: None,
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
        // A recall dated by the caller ranks as of then; otherwise as of now.
        let as_of = match self.as_of {
            Some(stated) => stated,
            None => crate::memory::now_ms()?,
        };
        rank(scope, query, limit, as_of, semantic.as_deref())
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

/// Ids by their value, highest first; ties go by id, so a recall repeats.
fn descending(scored: impl Iterator<Item = (String, f64)>) -> Vec<String> {
    let mut scored: Vec<(String, f64)> = scored.collect();
    scored.sort_by(|a, b| b.1.total_cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
    scored.into_iter().map(|(id, _)| id).collect()
}

/// Each id's position in `ordered`, first at 0.
fn positions(ordered: Vec<String>) -> HashMap<String, usize> {
    ordered
        .into_iter()
        .enumerate()
        .map(|(position, id)| (id, position))
        .collect()
}

/// What one ordering adds to a memory's fused score: the reciprocal of k plus
/// the memory's rank in that ordering.
fn fused(position: usize) -> f64 {
    (RRF_K + RRF_FIRST_RANK + position as f64).recip()
}

/// The memories visible in `scope` at `as_of`, ordered by the reciprocal rank
/// fusion of four orderings: full-text rank, closeness of meaning (when an
/// embedding was asked), confidence and recency. Fusing ranks rather than
/// adding scores needs no weight between them and no decay rate, so none is
/// stated here.
fn rank(
    scope: &MemoryScope,
    query: &str,
    limit: usize,
    as_of: i64,
    semantic: Option<&[(String, f64)]>,
) -> Result<Vec<RankedCandidate>, String> {
    let semantic: HashMap<String, f64> = match semantic {
        Some(scores) => scores.iter().cloned().collect(),
        None => HashMap::new(),
    };
    let (scope, terms) = (scope.clone(), tsquery(query));
    let semantic_ids = semantic.keys().cloned().collect::<Vec<_>>();
    // id -> (full-text rank when the query has words, confidence, updated_at)
    let rows: HashMap<String, (Option<f64>, f64, i64)> = run_db(move |db| async move {
        let lexical_score = if terms.is_empty() {
            Expr::cust("NULL::float8")
        } else {
            Expr::cust_with_values(
                "ts_rank(search, to_tsquery('simple', $1))::float8",
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
            .into_tuple::<(String, Option<f64>, f64, i64)>()
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
                rows.entry(id).or_insert((None, confidence, updated_at));
            }
        }
        Ok(rows)
    })?;
    let lexical = positions(descending(
        rows.iter()
            .filter_map(|(id, (lexical, _, _))| lexical.map(|score| (id.clone(), score))),
    ));
    let semantic =
        positions(descending(rows.keys().filter_map(|id| {
            semantic.get(id).map(|score| (id.clone(), *score))
        })));
    let confidence = positions(descending(
        rows.iter()
            .map(|(id, (_, confidence, _))| (id.clone(), *confidence)),
    ));
    let recency = positions(descending(
        rows.iter()
            .map(|(id, (_, _, updated_at))| (id.clone(), *updated_at as f64)),
    ));
    let mut ranked = Vec::new();
    for id in rows.into_keys() {
        let components = ScoreComponents {
            lexical_position: lexical.get(&id).copied(),
            semantic_position: semantic.get(&id).copied(),
            confidence_position: confidence[&id],
            recency_position: recency[&id],
        };
        let score = [
            components.lexical_position,
            components.semantic_position,
            Some(components.confidence_position),
            Some(components.recency_position),
        ]
        .into_iter()
        .flatten()
        .map(fused)
        .sum();
        ranked.push(RankedCandidate {
            id,
            score,
            components,
        });
    }
    ranked.sort_by(|a, b| b.score.total_cmp(&a.score).then_with(|| a.id.cmp(&b.id)));
    ranked.truncate(limit);
    Ok(ranked)
}
