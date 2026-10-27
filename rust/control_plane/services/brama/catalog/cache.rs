//! Remembering the model catalogue between calls and across restarts, keyed
//! so one set of credentials never reads another's.
//!
//! Split out of `control_plane/services/brama.rs`, which had grown past the
//! module line cap.

use super::ModelCatalog;
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
/// `brama-models-<sha256(endpoint + bearer + agent scope)>.json`. The
/// caller scope is part of the key because Brama filters and discovers models
/// for the signed agent. The in-memory CACHE above only lives for one process,
/// so a fresh `jeden` start can reuse the matching scoped catalog.
fn disk_cache_path(key: &str) -> std::path::PathBuf {
    use sha2::Digest;
    let digest = sha2::Sha256::digest(key.as_bytes());
    crate::dirs_home().join(format!(
        ".jeden/cache/brama-models-{}.json",
        hex::encode(digest)
    ))
}

/// The catalogue another process kept, when there is one. An entry that is
/// there but cannot be read or does not parse is said on stderr, as a write
/// that failed is, and the catalogue is then fetched again.
pub(super) fn read_disk_cache(key: &str) -> Option<(ModelCatalog, Option<String>, u64)> {
    #[derive(serde::Deserialize)]
    #[serde(rename_all = "camelCase")]
    struct Kept {
        fetched_at_ms: u64,
        etag: Option<String>,
        catalog: ModelCatalog,
    }
    let path = disk_cache_path(key);
    let text = match std::fs::read_to_string(&path) {
        Ok(text) => text,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return None,
        Err(error) => {
            eprintln!(
                "jeden: the kept model catalogue at {} cannot be read: {error}",
                path.display()
            );
            return None;
        }
    };
    match serde_json::from_str::<Kept>(&text) {
        Ok(kept) => Some((kept.catalog, kept.etag, kept.fetched_at_ms)),
        Err(error) => {
            eprintln!(
                "jeden: the kept model catalogue at {} does not parse: {error}",
                path.display()
            );
            None
        }
    }
}

/// Keep `catalog` for the next process. The cache only spares a request to
/// Brama, so a failure is said on stderr and the answer stands; left silent,
/// every start paid for the catalogue again with nobody knowing why.
pub(super) fn write_disk_cache(key: &str, catalog: &ModelCatalog, etag: Option<&str>) {
    let path = disk_cache_path(key);
    let written = crate::clock::unix_millis()
        .map_err(|error| format!("the time it was fetched cannot be read: {error}"))
        .and_then(|fetched_at_ms| {
            let payload = serde_json::json!({
                "fetchedAtMs": fetched_at_ms,
                "etag": etag,
                "catalog": catalog,
            });
            // The shared durable replace stages each write in a file of its
            // own, so concurrent jeden processes never rename a mix of two.
            crate::tool_runtime::runtime_ops::durable::replace(&path, payload.to_string().as_bytes())
        });
    if let Err(error) = written {
        eprintln!(
            "jeden: the model catalogue is not kept at {}: {error}",
            path.display()
        );
    }
}

/// The cache key of one catalog answer: the endpoint, the bearer it was asked
/// with and the agent that signed it. Credentials that cannot be read are refused,
/// since a key built without them would share another caller's answer.
pub(super) fn catalog_cache_key(endpoint: &str, bearer: Option<&str>) -> Result<String, String> {
    use sha2::Digest;
    let bearer_scope = match bearer {
        Some(token) => hex::encode(sha2::Sha256::digest(token.as_bytes())),
        None => "anonymous".to_string(),
    };
    let agent_scope = match caller_credentials()? {
        Some((agent_id, _)) => agent_id,
        None => "unsigned".to_string(),
    };
    Ok(format!("{endpoint}\u{0}bearer={bearer_scope}\u{0}agent={agent_scope}"))
}
