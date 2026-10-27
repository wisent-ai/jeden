use super::tenant::TenantId;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::fs::{self, OpenOptions};
use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

#[derive(Debug, Clone, PartialEq)]
pub enum IdempotencyDecision {
    Start,
    Reattach { request_id: String },
    Completed { request_id: String, result: Value },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum IdempotencyError {
    InvalidKey,
    Conflict,
    NotActive,
    Storage(String),
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "state", rename_all = "snake_case")]
enum DurableState {
    Active { request_id: String },
    Completed { request_id: String, result: Value },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct DurableRecord {
    tenant: String,
    key_digest: String,
    request_digest: String,
    state: DurableState,
}

#[derive(Debug, Clone)]
pub struct IdempotencyStore {
    root: PathBuf,
    lock: Arc<Mutex<()>>,
}

impl IdempotencyStore {
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self {
            root: root.into(),
            lock: Arc::new(Mutex::new(())),
        }
    }

    pub fn request_digest(payload: &[u8]) -> String {
        hex::encode(Sha256::digest(payload))
    }

    pub fn begin(
        &self,
        tenant: &TenantId,
        key: &str,
        request_digest: &str,
        request_id: &str,
    ) -> Result<IdempotencyDecision, IdempotencyError> {
        validate(key, request_digest, request_id)?;
        let _guard = self
            .lock
            .lock()
            .map_err(|poisoned| IdempotencyError::Storage(format!("idempotency lock: {poisoned}")))?;
        let path = self.record_path(tenant, key);
        if let Some(record) = read_existing_record(&path)? {
            if record.tenant != tenant.as_str() || record.request_digest != request_digest {
                return Err(IdempotencyError::Conflict);
            }
            return Ok(match record.state {
                DurableState::Active { request_id } => IdempotencyDecision::Reattach { request_id },
                DurableState::Completed { request_id, result } => {
                    IdempotencyDecision::Completed { request_id, result }
                }
            });
        }
        let record = DurableRecord {
            tenant: tenant.as_str().to_owned(),
            key_digest: digest_key(key),
            request_digest: request_digest.to_owned(),
            state: DurableState::Active {
                request_id: request_id.to_owned(),
            },
        };
        atomic_write(&path, &record)?;
        Ok(IdempotencyDecision::Start)
    }

    pub fn complete(
        &self,
        tenant: &TenantId,
        key: &str,
        request_digest: &str,
        result: Value,
    ) -> Result<(), IdempotencyError> {
        let _guard = self
            .lock
            .lock()
            .map_err(|poisoned| IdempotencyError::Storage(format!("idempotency lock: {poisoned}")))?;
        let path = self.record_path(tenant, key);
        let mut record = read_record(&path)?;
        if record.tenant != tenant.as_str() || record.request_digest != request_digest {
            return Err(IdempotencyError::Conflict);
        }
        let request_id = match record.state {
            DurableState::Active { request_id } => request_id,
            DurableState::Completed { .. } => return Ok(()),
        };
        record.state = DurableState::Completed { request_id, result };
        atomic_write(&path, &record)
    }

    pub fn abandon(
        &self,
        tenant: &TenantId,
        key: &str,
        request_digest: &str,
        request_id: &str,
    ) -> Result<(), IdempotencyError> {
        let _guard = self
            .lock
            .lock()
            .map_err(|poisoned| IdempotencyError::Storage(format!("idempotency lock: {poisoned}")))?;
        let path = self.record_path(tenant, key);
        let record = read_record(&path)?;
        let matches = record.tenant == tenant.as_str()
            && record.request_digest == request_digest
            && matches!(record.state, DurableState::Active { request_id: ref active } if active == request_id);
        if !matches {
            return Err(IdempotencyError::Conflict);
        }
        fs::remove_file(&path).map_err(|error| {
            IdempotencyError::Storage(format!(
                "idempotency record {} cannot be removed: {error}",
                path.display()
            ))
        })?;
        if let Some(parent) = path.parent() {
            sync_directory(parent)?;
        }
        Ok(())
    }

    pub fn active_request(
        &self,
        tenant: &TenantId,
        key: &str,
    ) -> Result<Option<String>, IdempotencyError> {
        let path = self.record_path(tenant, key);
        let Some(record) = read_existing_record(&path)? else {
            return Ok(None);
        };
        if record.tenant != tenant.as_str() {
            return Ok(None);
        }
        Ok(match record.state {
            DurableState::Active { request_id } => Some(request_id),
            DurableState::Completed { .. } => None,
        })
    }

    fn record_path(&self, tenant: &TenantId, key: &str) -> PathBuf {
        let tenant_digest = digest_key(tenant.as_str());
        self.root
            .join(tenant_digest)
            .join(format!("{}.json", digest_key(key)))
    }
}

/// A key is any non-empty text (the record is named by its digest, so its
/// length never reaches the file system), the request id is non-empty, and
/// the request digest is a hex SHA-256 as request_digest() writes it.
fn validate(key: &str, request_digest: &str, request_id: &str) -> Result<(), IdempotencyError> {
    let digest = hex::decode(request_digest)
        .is_ok_and(|bytes| bytes.len() == <Sha256 as Digest>::output_size());
    if key.is_empty() || request_id.is_empty() || !digest {
        Err(IdempotencyError::InvalidKey)
    } else {
        Ok(())
    }
}

fn digest_key(value: &str) -> String {
    hex::encode(Sha256::digest(value.as_bytes()))
}

fn read_record(path: &Path) -> Result<DurableRecord, IdempotencyError> {
    read_existing_record(path)?.ok_or_else(|| {
        IdempotencyError::Storage(format!(
            "idempotency record {} does not exist",
            path.display()
        ))
    })
}

/// The record at `path`, or `None` only when it does not exist. Testing
/// `exists()` first answered false for a record that could not be stat'ed, so
/// `begin` started the request again and overwrote the record of one already
/// running or completed; any failure but NotFound is a storage error.
fn read_existing_record(path: &Path) -> Result<Option<DurableRecord>, IdempotencyError> {
    let mut file = match OpenOptions::new().read(true).open(path) {
        Ok(file) => file,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => {
            return Err(IdempotencyError::Storage(format!(
                "idempotency record {} cannot be opened: {error}",
                path.display()
            )))
        }
    };
    let mut bytes = Vec::new();
    file.read_to_end(&mut bytes).map_err(|error| {
        IdempotencyError::Storage(format!(
            "idempotency record {} cannot be read: {error}",
            path.display()
        ))
    })?;
    serde_json::from_slice(&bytes).map(Some).map_err(|error| {
        IdempotencyError::Storage(format!(
            "idempotency record {} is not a record: {error}",
            path.display()
        ))
    })
}

/// `record` over `path` through the shared durable replace: a staged file of
/// this write's own, so two requests recording one key never write into one
/// shared staging file.
fn atomic_write(path: &Path, record: &DurableRecord) -> Result<(), IdempotencyError> {
    let bytes = serde_json::to_vec(record).map_err(|error| {
        IdempotencyError::Storage(format!(
            "idempotency record {} cannot be encoded: {error}",
            path.display()
        ))
    })?;
    crate::tool_runtime::runtime_ops::durable::replace(path, &bytes)
        .map_err(IdempotencyError::Storage)
}

/// Flushes `directory`'s entries, so a record written or removed in it
/// survives a crash; a directory that cannot be opened or flushed fails the
/// operation instead of leaving its durability unknown.
fn sync_directory(directory: &Path) -> Result<(), IdempotencyError> {
    OpenOptions::new()
        .read(true)
        .open(directory)
        .and_then(|opened| opened.sync_all())
        .map_err(|error| {
            IdempotencyError::Storage(format!(
                "idempotency directory {} cannot be flushed: {error}",
                directory.display()
            ))
        })
}
