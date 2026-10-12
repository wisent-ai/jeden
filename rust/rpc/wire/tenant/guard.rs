//! Keeping one tenant inside its own share of the machine.
//!
//! Split out of `rpc/wire/tenant.rs`, which had grown past the module line
//! cap.

use super::{TenantError, TenantId, TenantLimits, TenantPrincipal, Usage};
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::path::{Component, Path, PathBuf};
use std::sync::{Arc, Mutex};

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
        // Admission recovers a poisoned usage map, so the release must too:
        // a permit that is never given back holds the tenant's slot forever.
        let mut usage = match self.usage.lock() {
            Ok(usage) => usage,
            Err(poisoned) => poisoned.into_inner(),
        };
        let entry = usage.entry(self.tenant.clone()).or_default();
        entry.active_requests = entry.active_requests.saturating_sub(1);
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
                quota: "activeRequests",
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
            return Err(TenantError::QuotaExceeded { quota: "sessions" });
        }
        entry.sessions += 1;
        Ok(())
    }

    /// Gives back the session unit `register_session` took. A panic elsewhere
    /// cannot break a counter, so a poisoned lock is recovered rather than
    /// leaving the unit held and the tenant over its quota for good.
    pub fn release_session(&self, tenant: &TenantId) {
        let mut usage = match self.usage.lock() {
            Ok(usage) => usage,
            Err(poisoned) => poisoned.into_inner(),
        };
        let entry = usage.entry(tenant.clone()).or_default();
        entry.sessions = entry.sessions.saturating_sub(1);
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
                quota: "storedBytes",
            })?;
        if updated > self.limits.max_stored_bytes {
            return Err(TenantError::QuotaExceeded {
                quota: "storedBytes",
            });
        }
        entry.stored_bytes = updated;
        Ok(())
    }
}
