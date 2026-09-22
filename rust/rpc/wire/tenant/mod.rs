mod guard;

pub use guard::TenantGuard;

use super::tls::VerifiedPeer;
use std::collections::HashMap;
use std::path::{Component, Path, PathBuf};
use std::sync::{Arc, RwLock};

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
    /// Absolute, canonical host directories this principal may read and
    /// continue sessions inside. Empty means the historical behaviour: the
    /// principal only ever sees the scratch workspaces this daemon made for it.
    pub workspaces: Vec<PathBuf>,
}

impl TenantPrincipal {
    pub fn workspaces(&self) -> &[PathBuf] {
        &self.workspaces
    }

    /// True when `path` resolves inside one granted workspace. A principal with
    /// no grant never matches, so an absent `workspaces` key cannot widen
    /// anything. Both sides are canonicalised so a symlink or `..` cannot
    /// escape the grant; an unresolvable path is refused rather than assumed.
    pub fn grants_path(&self, path: &Path) -> bool {
        if self.workspaces.is_empty() {
            return false;
        }
        if path
            .components()
            .any(|part| matches!(part, Component::ParentDir))
        {
            return false;
        }
        let Ok(candidate) = path.canonicalize() else {
            return false;
        };
        self.workspaces
            .iter()
            .any(|root| candidate.starts_with(root))
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TenantError {
    IdentityNotMapped,
    AccessDenied,
    InvalidStorageKey,
    InvalidWorkspace(String),
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
pub(super) struct Usage {
    pub(super) active_requests: usize,
    pub(super) sessions: usize,
    pub(super) stored_bytes: u64,
}

#[derive(Debug, Clone)]
pub struct TenantDirectory {
    mappings: Arc<RwLock<HashMap<String, TenantPrincipal>>>,
}

impl Default for TenantDirectory {
    fn default() -> Self {
        Self::new()
    }
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
        workspaces: Vec<PathBuf>,
    ) -> Result<(), TenantError> {
        let san = san.into();
        let principal = validate_id(principal.into())?;
        let tenant = validate_id(tenant.into())?;
        let mut granted = Vec::with_capacity(workspaces.len());
        for workspace in workspaces {
            // A grant is resolved once, at map time, so every later containment
            // check is a pure component comparison against a real directory.
            if !workspace.is_absolute()
                || workspace
                    .components()
                    .any(|part| matches!(part, Component::ParentDir))
            {
                return Err(TenantError::InvalidWorkspace(format!(
                    "workspace {} must be an absolute path without ..",
                    workspace.display()
                )));
            }
            let resolved = workspace.canonicalize().map_err(|error| {
                TenantError::InvalidWorkspace(format!(
                    "workspace {} is not readable: {}",
                    workspace.display(),
                    error
                ))
            })?;
            if !resolved.is_dir() {
                return Err(TenantError::InvalidWorkspace(format!(
                    "workspace {} is not an existing directory",
                    workspace.display()
                )));
            }
            granted.push(resolved);
        }
        self.mappings
            .write()
            .map_err(|_| TenantError::StorageUnavailable)?
            .insert(
                san,
                TenantPrincipal {
                    principal: PrincipalId(principal),
                    tenant: TenantId(tenant),
                    workspaces: granted,
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
