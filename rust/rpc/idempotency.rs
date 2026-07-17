use super::tenant::TenantId;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::fs::{self, OpenOptions};
use std::io::{Read, Write};
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
    version: u32,
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
            .map_err(|_| IdempotencyError::Storage("idempotency lock poisoned".into()))?;
        let path = self.record_path(tenant, key);
        if path.exists() {
            let record = read_record(&path)?;
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
            version: 1,
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
            .map_err(|_| IdempotencyError::Storage("idempotency lock poisoned".into()))?;
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
            .map_err(|_| IdempotencyError::Storage("idempotency lock poisoned".into()))?;
        let path = self.record_path(tenant, key);
        let record = read_record(&path)?;
        let matches = record.tenant == tenant.as_str()
            && record.request_digest == request_digest
            && matches!(record.state, DurableState::Active { request_id: ref active } if active == request_id);
        if !matches {
            return Err(IdempotencyError::Conflict);
        }
        fs::remove_file(&path).map_err(storage)?;
        if let Some(parent) = path.parent() {
            if let Ok(directory) = OpenOptions::new().read(true).open(parent) {
                let _ = directory.sync_all();
            }
        }
        Ok(())
    }

    pub fn active_request(
        &self,
        tenant: &TenantId,
        key: &str,
    ) -> Result<Option<String>, IdempotencyError> {
        let path = self.record_path(tenant, key);
        if !path.exists() {
            return Ok(None);
        }
        let record = read_record(&path)?;
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

fn validate(key: &str, request_digest: &str, request_id: &str) -> Result<(), IdempotencyError> {
    if key.is_empty()
        || key.len() > 512
        || request_id.is_empty()
        || request_digest.len() != 64
        || !request_digest.bytes().all(|byte| byte.is_ascii_hexdigit())
    {
        Err(IdempotencyError::InvalidKey)
    } else {
        Ok(())
    }
}

fn digest_key(value: &str) -> String {
    hex::encode(Sha256::digest(value.as_bytes()))
}

fn read_record(path: &Path) -> Result<DurableRecord, IdempotencyError> {
    let mut file = OpenOptions::new().read(true).open(path).map_err(storage)?;
    let mut bytes = Vec::new();
    file.read_to_end(&mut bytes).map_err(storage)?;
    serde_json::from_slice(&bytes).map_err(|error| IdempotencyError::Storage(error.to_string()))
}

fn atomic_write(path: &Path, record: &DurableRecord) -> Result<(), IdempotencyError> {
    let parent = path
        .parent()
        .ok_or_else(|| IdempotencyError::Storage("record has no parent".into()))?;
    fs::create_dir_all(parent).map_err(storage)?;
    let temporary = path.with_extension("tmp");
    let bytes =
        serde_json::to_vec(record).map_err(|error| IdempotencyError::Storage(error.to_string()))?;
    let mut file = OpenOptions::new()
        .create(true)
        .truncate(true)
        .write(true)
        .open(&temporary)
        .map_err(storage)?;
    file.write_all(&bytes).map_err(storage)?;
    file.sync_all().map_err(storage)?;
    fs::rename(&temporary, path).map_err(storage)?;
    if let Ok(directory) = OpenOptions::new().read(true).open(parent) {
        let _ = directory.sync_all();
    }
    Ok(())
}

fn storage(error: std::io::Error) -> IdempotencyError {
    IdempotencyError::Storage(error.to_string())
}
