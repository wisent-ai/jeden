use crate::fleet::{run_db, sql};
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
    let records: Vec<(String, String)> = run_db(|client| {
        Ok(client
            .query(
                "SELECT id,text FROM memories WHERE status='active' AND NOT tombstone AND valid_to IS NULL ORDER BY id",
                &[],
            )
            .map_err(sql)?
            .into_iter()
            .map(|row| (row.get(0), row.get(1)))
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
    let dimensions = vectors.first().map(Vec::len).unwrap_or(0) as i64;
    if vectors
        .iter()
        .any(|vector| vector.len() as i64 != dimensions || vector.iter().any(|v| !v.is_finite()))
    {
        return Err("embedding provider returned invalid or inconsistent vectors".into());
    }
    let now = crate::memory::now_ms();
    let model = provider.name().to_owned();
    let rows = records
        .into_iter()
        .zip(vectors)
        .map(|((id, text), vector)| {
            serde_json::to_string(&vector)
                .map(|json| (id, json, content_hash(&text)))
                .map_err(|e| e.to_string())
        })
        .collect::<Result<Vec<_>, _>>()?;
    run_db(move |client| {
        let mut tx = client.transaction().map_err(sql)?;
        tx.execute("DELETE FROM memory_embeddings", &[])
            .map_err(sql)?;
        for (id, json, hash) in &rows {
            tx.execute(
                "INSERT INTO memory_embeddings(memory_id,model,dimensions,vector_json,content_hash,updated_at)
                 VALUES($1,$2,$3,$4,$5,$6)",
                &[id, &model, &dimensions, json, hash, &now],
            )
            .map_err(sql)?;
        }
        tx.commit().map_err(sql)?;
        Ok(rows.len())
    })
}

pub(crate) fn health(provider: Option<&dyn EmbeddingProvider>) -> Result<EmbeddingHealth, String> {
    let rows: Vec<(String, String)> = run_db(|client| {
        Ok(client
            .query(
                "SELECT e.content_hash,m.text FROM memory_embeddings e JOIN memories m ON m.id=e.memory_id",
                &[],
            )
            .map_err(sql)?
            .into_iter()
            .map(|row| (row.get(0), row.get(1)))
            .collect())
    })?;
    let indexed = rows.len();
    let stale = rows
        .iter()
        .filter(|(stored, text)| *stored != content_hash(text))
        .count();
    let available = provider.map(EmbeddingProvider::available).unwrap_or(false);
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
    let rows: Vec<(String, String)> = run_db(move |client| {
        Ok(client
            .query(
                "SELECT memory_id,vector_json FROM memory_embeddings WHERE dimensions=$1",
                &[&dimensions],
            )
            .map_err(sql)?
            .into_iter()
            .map(|row| (row.get(0), row.get(1)))
            .collect())
    })?;
    let mut result = Vec::new();
    for (id, json) in rows {
        let vector: Vec<f32> = serde_json::from_str(&json).map_err(|e| e.to_string())?;
        result.push((id, cosine(query, &vector)));
    }
    Ok(result)
}

fn cosine(a: &[f32], b: &[f32]) -> f64 {
    let (mut dot, mut aa, mut bb) = (0.0_f64, 0.0_f64, 0.0_f64);
    for (&x, &y) in a.iter().zip(b) {
        let (x, y) = (x as f64, y as f64);
        dot += x * y;
        aa += x * x;
        bb += y * y;
    }
    if aa == 0.0 || bb == 0.0 {
        0.0
    } else {
        (dot / (aa.sqrt() * bb.sqrt())).clamp(-1.0, 1.0)
    }
}
