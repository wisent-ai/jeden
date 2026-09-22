//! Remembering the model catalogue between calls and across restarts, keyed
//! so one set of credentials never reads another's.
//!
//! Split out of `control_plane/services/brama.rs`, which had grown past the
//! module line cap.

use super::super::transport::SecretRef;
use super::ModelCatalog;
use crate::control_plane::now_ms;
use crate::control_plane::services::brama::auth::caller_credentials;
use std::collections::HashMap;
use std::sync::{LazyLock, Mutex};
use std::time::Instant;

#[derive(Clone)]
pub(super) struct CachedCatalog {
    pub(super) catalog: ModelCatalog,
    pub(super) etag: Option<String>,
    pub(super) fetched: Instant,
}
pub(super) static CACHE: LazyLock<Mutex<HashMap<String, CachedCatalog>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

/// On-disk catalog cache shared across processes: `~/.jeden/cache/`
/// `brama-models-<sha256(endpoint + bearer + agent scope)[:16]>.json`. The
/// caller scope is part of the key because Brama filters and discovers models
/// for the signed agent. The in-memory CACHE above only lives for one process,
/// so a fresh `jeden` start can reuse the matching scoped catalog.
fn disk_cache_path(key: &str) -> std::path::PathBuf {
    use sha2::Digest;
    let digest = sha2::Sha256::digest(key.as_bytes());
    crate::dirs_home().join(format!(
        ".jeden/cache/brama-models-{}.json",
        &hex::encode(digest)[..16]
    ))
}

pub(super) fn read_disk_cache(key: &str) -> Option<(ModelCatalog, Option<String>, u64)> {
    let text = std::fs::read_to_string(disk_cache_path(key)).ok()?;
    let value: serde_json::Value = serde_json::from_str(&text).ok()?;
    let fetched_ms = value.get("fetchedAtMs")?.as_u64()?;
    let etag = value
        .get("etag")
        .and_then(serde_json::Value::as_str)
        .map(str::to_string);
    let catalog: ModelCatalog = serde_json::from_value(value.get("catalog")?.clone()).ok()?;
    Some((catalog, etag, fetched_ms))
}

pub(super) fn write_disk_cache(key: &str, catalog: &ModelCatalog, etag: Option<&str>) {
    let path = disk_cache_path(key);
    let Some(parent) = path.parent() else {
        return;
    };
    if std::fs::create_dir_all(parent).is_err() {
        return;
    }
    let payload = serde_json::json!({
        "fetchedAtMs": now_ms(),
        "etag": etag,
        "catalog": catalog,
    });
    // tmp + rename keeps the entry atomic for concurrent jeden processes.
    let tmp = path.with_extension("json.tmp");
    if std::fs::write(&tmp, payload.to_string()).is_ok() {
        let _ = std::fs::rename(&tmp, &path);
    }
}

pub(super) fn catalog_cache_key(endpoint: &str, authorization: Option<&SecretRef>) -> String {
    use sha2::Digest;
    let bearer_scope = authorization
        .and_then(SecretRef::resolve)
        .map(|token| hex::encode(sha2::Sha256::digest(token.as_bytes())))
        .unwrap_or_else(|| "anonymous".into());
    let agent_scope = caller_credentials()
        .map(|(agent_id, _)| agent_id)
        .unwrap_or_else(|| "unsigned".into());
    format!("{endpoint}\u{0}bearer={bearer_scope}\u{0}agent={agent_scope}")
}
