use super::tls::VerifiedPeer;
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::path::{Component, Path, PathBuf};
use std::sync::{Arc, Mutex, RwLock};

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct PrincipalId(String);

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct TenantId(String);

impl PrincipalId {
    pub fn as_str(&self) -> &str {
        &self.0
    }
}
impl TenantId {
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TenantPrincipal {
    pub principal: PrincipalId,
    pub tenant: TenantId,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TenantError {
    IdentityNotMapped,
    AccessDenied,
    InvalidStorageKey,
    StorageUnavailable,
    QuotaExceeded { retry_after_millis: u64 },
}

#[derive(Debug, Clone)]
pub struct TenantLimits {
    pub max_active_requests: usize,
    pub max_sessions: usize,
    pub max_stored_bytes: u64,
}

#[derive(Debug, Default, Clone)]
struct Usage {
    active_requests: usize,
    sessions: usize,
    stored_bytes: u64,
}

#[derive(Debug, Clone)]
pub struct TenantDirectory {
    mappings: Arc<RwLock<HashMap<String, TenantPrincipal>>>,
}

impl TenantDirectory {
    pub fn new() -> Self {
        Self {
            mappings: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    pub fn map_san(
        &self,
        san: impl Into<String>,
        principal: impl Into<String>,
        tenant: impl Into<String>,
    ) -> Result<(), TenantError> {
        let san = san.into();
        let principal = validate_id(principal.into())?;
        let tenant = validate_id(tenant.into())?;
        self.mappings
            .write()
            .map_err(|_| TenantError::StorageUnavailable)?
            .insert(
                san,
                TenantPrincipal {
                    principal: PrincipalId(principal),
                    tenant: TenantId(tenant),
                },
            );
        Ok(())
    }

    pub fn resolve(&self, peer: &VerifiedPeer) -> Result<TenantPrincipal, TenantError> {
        let mappings = self
            .mappings
            .read()
            .map_err(|_| TenantError::StorageUnavailable)?;
        peer.certificate
            .uri_sans
            .iter()
            .chain(peer.certificate.dns_sans.iter())
            .find_map(|san| mappings.get(san).cloned())
            .ok_or(TenantError::IdentityNotMapped)
    }
}

#[derive(Debug, Clone)]
pub struct TenantGuard {
    root: PathBuf,
    limits: TenantLimits,
    usage: Arc<Mutex<HashMap<TenantId, Usage>>>,
}

pub struct ActiveRequestPermit {
    tenant: TenantId,
    usage: Arc<Mutex<HashMap<TenantId, Usage>>>,
}

impl Drop for ActiveRequestPermit {
    fn drop(&mut self) {
        if let Ok(mut usage) = self.usage.lock() {
            let entry = usage.entry(self.tenant.clone()).or_default();
            entry.active_requests = entry.active_requests.saturating_sub(1);
        }
    }
}

impl TenantGuard {
    pub fn new(root: impl Into<PathBuf>, limits: TenantLimits) -> Self {
        Self {
            root: root.into(),
            limits,
            usage: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    pub fn tenant_root(&self, tenant: &TenantId) -> PathBuf {
        let mut digest = Sha256::new();
        digest.update(tenant.as_str().as_bytes());
        self.root.join(hex::encode(digest.finalize()))
    }

    pub fn scoped_path(&self, tenant: &TenantId, relative: &Path) -> Result<PathBuf, TenantError> {
        if relative.as_os_str().is_empty()
            || relative.is_absolute()
            || relative
                .components()
                .any(|part| !matches!(part, Component::Normal(_)))
        {
            return Err(TenantError::InvalidStorageKey);
        }
        Ok(self.tenant_root(tenant).join(relative))
    }

    pub fn authorize_owner(
        &self,
        caller: &TenantPrincipal,
        owner: &TenantId,
    ) -> Result<(), TenantError> {
        if &caller.tenant == owner {
            Ok(())
        } else {
            Err(TenantError::AccessDenied)
        }
    }

    pub fn reserve_request(&self, tenant: &TenantId) -> Result<ActiveRequestPermit, TenantError> {
        let mut usage = self
            .usage
            .lock()
            .map_err(|_| TenantError::StorageUnavailable)?;
        let entry = usage.entry(tenant.clone()).or_default();
        if entry.active_requests >= self.limits.max_active_requests {
            return Err(TenantError::QuotaExceeded {
                retry_after_millis: 250,
            });
        }
        entry.active_requests += 1;
        Ok(ActiveRequestPermit {
            tenant: tenant.clone(),
            usage: self.usage.clone(),
        })
    }

    pub fn register_session(&self, tenant: &TenantId) -> Result<(), TenantError> {
        let mut usage = self
            .usage
            .lock()
            .map_err(|_| TenantError::StorageUnavailable)?;
        let entry = usage.entry(tenant.clone()).or_default();
        if entry.sessions >= self.limits.max_sessions {
            return Err(TenantError::QuotaExceeded {
                retry_after_millis: 1000,
            });
        }
        entry.sessions += 1;
        Ok(())
    }

    pub fn release_session(&self, tenant: &TenantId) -> Result<(), TenantError> {
        let mut usage = self
            .usage
            .lock()
            .map_err(|_| TenantError::StorageUnavailable)?;
        let entry = usage.entry(tenant.clone()).or_default();
        entry.sessions = entry.sessions.saturating_sub(1);
        Ok(())
    }

    pub fn account_bytes(&self, tenant: &TenantId, delta: u64) -> Result<(), TenantError> {
        let mut usage = self
            .usage
            .lock()
            .map_err(|_| TenantError::StorageUnavailable)?;
        let entry = usage.entry(tenant.clone()).or_default();
        let updated = entry
            .stored_bytes
            .checked_add(delta)
            .ok_or(TenantError::QuotaExceeded {
                retry_after_millis: 1000,
            })?;
        if updated > self.limits.max_stored_bytes {
            return Err(TenantError::QuotaExceeded {
                retry_after_millis: 1000,
            });
        }
        entry.stored_bytes = updated;
        Ok(())
    }
}

fn validate_id(value: String) -> Result<String, TenantError> {
    if value.is_empty()
        || value.len() > 255
        || !value.bytes().all(|byte| {
            byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.' | b':' | b'/')
        })
    {
        Err(TenantError::IdentityNotMapped)
    } else {
        Ok(value)
    }
}
