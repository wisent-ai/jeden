use crate::fleet::{run_db, sql};
use crate::memory::store::entities::{embedding, memory};
use sea_orm::{
    ActiveValue::Set, ColumnTrait, EntityTrait, QueryFilter, QueryOrder, TransactionTrait,
};
use serde::Serialize;
use sha2::{Digest, Sha256};

pub trait EmbeddingProvider: Send + Sync {
    fn name(&self) -> &str;
    fn embed(&self, texts: &[String]) -> Result<Vec<Vec<f32>>, String>;
    fn available(&self) -> bool {
        true
    }
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct EmbeddingHealth {
    pub available: bool,
    pub provider: Option<String>,
    pub indexed: usize,
    pub stale: usize,
    pub mode: String,
}

pub(super) fn content_hash(text: &str) -> String {
    hex::encode(Sha256::digest(text.as_bytes()))
}

pub(crate) fn rebuild(provider: &dyn EmbeddingProvider) -> Result<usize, String> {
    if !provider.available() {
        return Err(format!(
            "embedding provider {} is unavailable",
            provider.name()
        ));
    }
    let records: Vec<(String, String)> = run_db(|db| async move {
        Ok(memory::Entity::find()
            .filter(memory::Column::Status.eq("active"))
            .filter(memory::Column::Tombstone.eq(false))
            .filter(memory::Column::ValidTo.is_null())
            .order_by_asc(memory::Column::Id)
            .all(&db)
            .await
            .map_err(sql)?
            .into_iter()
            .map(|row| (row.id, row.text))
            .collect())
    })?;
    let texts = records
        .iter()
        .map(|(_, text)| text.clone())
        .collect::<Vec<_>>();
    let vectors = provider.embed(&texts)?;
    if vectors.len() != records.len() {
        return Err("embedding provider returned a different vector count".into());
    }
    // Every vector is as wide as the first and holds only finite numbers.
    if let Some(first) = vectors.first() {
        let width = first.len();
        if vectors
            .iter()
            .any(|vector| vector.len() != width || vector.iter().any(|v| !v.is_finite()))
        {
            return Err("embedding provider returned invalid or inconsistent vectors".into());
        }
    }
    let now = crate::memory::now_ms()?;
    let model = provider.name().to_owned();
    let rows = records
        .into_iter()
        .zip(vectors)
        .map(|((id, text), vector)| {
            let dimensions = vector.len() as i64;
            serde_json::to_string(&vector)
                .map(|json| embedding::ActiveModel {
                    memory_id: Set(id),
                    model: Set(model.clone()),
                    dimensions: Set(dimensions),
                    vector_json: Set(json),
                    content_hash: Set(content_hash(&text)),
                    updated_at: Set(now),
                })
                .map_err(|e| e.to_string())
        })
        .collect::<Result<Vec<_>, _>>()?;
    let count = rows.len();
    run_db(move |db| async move {
        let tx = db.begin().await.map_err(sql)?;
        embedding::Entity::delete_many()
            .exec(&tx)
            .await
            .map_err(sql)?;
        if !rows.is_empty() {
            embedding::Entity::insert_many(rows)
                .exec_without_returning(&tx)
                .await
                .map_err(sql)?;
        }
        tx.commit().await.map_err(sql)
    })?;
    Ok(count)
}

pub(crate) fn health(provider: Option<&dyn EmbeddingProvider>) -> Result<EmbeddingHealth, String> {
    let (embeddings, texts) = run_db(|db| async move {
        let embeddings = embedding::Entity::find().all(&db).await.map_err(sql)?;
        let ids = embeddings
            .iter()
            .map(|row| row.memory_id.clone())
            .collect::<Vec<_>>();
        let texts = memory::Entity::find()
            .filter(memory::Column::Id.is_in(ids))
            .all(&db)
            .await
            .map_err(sql)?
            .into_iter()
            .map(|row| (row.id, row.text))
            .collect::<std::collections::HashMap<_, _>>();
        Ok((embeddings, texts))
    })?;
    let indexed = embeddings.len();
    let stale = embeddings
        .iter()
        .filter(|row| {
            texts
                .get(&row.memory_id)
                .is_none_or(|text| row.content_hash != content_hash(text))
        })
        .count();
    let available = provider.is_some_and(|provider| provider.available());
    Ok(EmbeddingHealth {
        available,
        provider: provider.map(|p| p.name().to_string()),
        indexed,
        stale,
        mode: if available {
            "hybrid".into()
        } else {
            "lexical-only".into()
        },
    })
}

pub(super) fn semantic_scores(query: &[f32]) -> Result<Vec<(String, f64)>, String> {
    if query.is_empty() || query.iter().any(|v| !v.is_finite()) {
        return Ok(Vec::new());
    }
    let dimensions = query.len() as i64;
    let rows = run_db(move |db| async move {
        embedding::Entity::find()
            .filter(embedding::Column::Dimensions.eq(dimensions))
            .all(&db)
            .await
            .map_err(sql)
    })?;
    let mut result = Vec::new();
    for row in rows {
        let vector: Vec<f32> = serde_json::from_str(&row.vector_json).map_err(|e| e.to_string())?;
        // A zero vector points nowhere, so it has no angle to the query and
        // takes no semantic score.
        if let Some(score) = cosine(query, &vector) {
            result.push((row.memory_id, score));
        }
    }
    Ok(result)
}

/// The cosine of the angle between two vectors, or `None` when either is a
/// zero vector, whose angle is undefined.
fn cosine(a: &[f32], b: &[f32]) -> Option<f64> {
    let dot: f64 = a
        .iter()
        .zip(b)
        .map(|(&x, &y)| f64::from(x) * f64::from(y))
        .sum();
    let norm = |vector: &[f32]| {
        vector
            .iter()
            .map(|&x| f64::from(x) * f64::from(x))
            .sum::<f64>()
            .sqrt()
    };
    let scale = norm(a) * norm(b);
    (scale.classify() != std::num::FpCategory::Zero).then(|| dot / scale)
}
