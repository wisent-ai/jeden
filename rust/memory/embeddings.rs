use rusqlite::{params, Connection};
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

pub(super) fn rebuild(
    conn: &mut Connection,
    provider: &dyn EmbeddingProvider,
) -> Result<usize, String> {
    if !provider.available() {
        return Err(format!(
            "embedding provider {} is unavailable",
            provider.name()
        ));
    }
    let records = {
        let mut stmt = conn.prepare(
            "SELECT id,text FROM memories WHERE status='active' AND tombstone=0 AND valid_to IS NULL ORDER BY id"
        ).map_err(|e| e.to_string())?;
        let rows = stmt
            .query_map([], |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
            })
            .map_err(|e| e.to_string())?
            .collect::<Result<Vec<_>, _>>()
            .map_err(|e| e.to_string())?;
        rows
    };
    let texts = records
        .iter()
        .map(|(_, text)| text.clone())
        .collect::<Vec<_>>();
    let vectors = provider.embed(&texts)?;
    if vectors.len() != records.len() {
        return Err("embedding provider returned a different vector count".into());
    }
    let dimensions = vectors.first().map(Vec::len).unwrap_or(0);
    if vectors
        .iter()
        .any(|vector| vector.len() != dimensions || vector.iter().any(|v| !v.is_finite()))
    {
        return Err("embedding provider returned invalid or inconsistent vectors".into());
    }
    let now = super::now_ms();
    let tx = conn.transaction().map_err(|e| e.to_string())?;
    tx.execute("DELETE FROM memory_embeddings", [])
        .map_err(|e| e.to_string())?;
    for ((id, text), vector) in records.iter().zip(vectors) {
        tx.execute(
            "INSERT INTO memory_embeddings(memory_id,model,dimensions,vector_json,content_hash,updated_at) VALUES(?1,?2,?3,?4,?5,?6)",
            params![id, provider.name(), dimensions as i64, serde_json::to_string(&vector).map_err(|e| e.to_string())?, content_hash(text), now],
        ).map_err(|e| e.to_string())?;
    }
    tx.commit().map_err(|e| e.to_string())?;
    Ok(records.len())
}

pub(super) fn health(
    conn: &Connection,
    provider: Option<&dyn EmbeddingProvider>,
) -> Result<EmbeddingHealth, String> {
    let mut stmt = conn.prepare("SELECT e.content_hash,m.text FROM memory_embeddings e JOIN memories m ON m.id=e.memory_id").map_err(|e| e.to_string())?;
    let rows = stmt
        .query_map([], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
        })
        .map_err(|e| e.to_string())?;
    let mut indexed = 0_usize;
    let mut stale = 0_usize;
    for row in rows {
        let (stored, text) = row.map_err(|e| e.to_string())?;
        indexed += 1;
        if stored != content_hash(&text) {
            stale += 1;
        }
    }
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

pub(super) fn semantic_scores(
    conn: &Connection,
    query: &[f32],
) -> Result<Vec<(String, f64)>, String> {
    if query.is_empty() || query.iter().any(|v| !v.is_finite()) {
        return Ok(Vec::new());
    }
    let mut stmt = conn
        .prepare("SELECT memory_id,vector_json FROM memory_embeddings WHERE dimensions=?1")
        .map_err(|e| e.to_string())?;
    let rows = stmt
        .query_map([query.len() as i64], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
        })
        .map_err(|e| e.to_string())?;
    let mut result = Vec::new();
    for row in rows {
        let (id, json) = row.map_err(|e| e.to_string())?;
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
