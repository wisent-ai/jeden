use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;
use std::path::PathBuf;
use std::time::{Duration, SystemTime};

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Principal {
    pub subject: String,
    pub tenant: Option<String>,
    pub kind: PrincipalKind,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PrincipalKind {
    User,
    Service,
    Worker,
    Extension,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FsGrant {
    pub read_roots: Vec<PathBuf>,
    pub write_roots: Vec<PathBuf>,
    pub max_file_bytes: u64,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NetworkGrant {
    pub hosts: BTreeSet<String>,
    pub ports: BTreeSet<u16>,
    pub allow_private: bool,
    pub max_redirects: u8,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SecretGrant {
    pub names: BTreeSet<String>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProcessGrant {
    pub programs: BTreeSet<String>,
    pub environment: BTreeSet<String>,
    pub inherit_stdio: bool,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ResourceLimits {
    pub cpu_seconds: u64,
    pub address_space_bytes: u64,
    pub open_files: u64,
    pub processes: u64,
    pub file_bytes: u64,
}

impl Default for ResourceLimits {
    fn default() -> Self {
        Self {
            cpu_seconds: 60,
            address_space_bytes: 2 * 1024 * 1024 * 1024,
            open_files: 256,
            processes: 64,
            file_bytes: 128 * 1024 * 1024,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SandboxRequirement {
    NotRequired,
    Enforced,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TelemetryPolicy {
    Disabled,
    LocalPrivate,
    ExportPrivate,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ExecutionGrant {
    pub principal: Principal,
    pub filesystem: FsGrant,
    pub network: NetworkGrant,
    pub secrets: SecretGrant,
    pub process: ProcessGrant,
    pub resource_limits: ResourceLimits,
    pub sandbox: SandboxRequirement,
    pub expires_at: SystemTime,
}

impl ExecutionGrant {
    /// Compatibility authority for host-owned builtins. PolicyEngine-issued restricted grants
    /// must replace this before running untrusted capability code.
    pub fn trusted_host(subject: impl Into<String>, root: PathBuf) -> Self {
        let root = root.canonicalize().unwrap_or(root);
        Self {
            principal: Principal {
                subject: subject.into(),
                tenant: None,
                kind: PrincipalKind::Service,
            },
            filesystem: FsGrant {
                read_roots: vec![root.clone()],
                write_roots: vec![root],
                max_file_bytes: 128 * 1024 * 1024,
            },
            network: NetworkGrant {
                hosts: BTreeSet::new(),
                ports: BTreeSet::new(),
                allow_private: false,
                max_redirects: 0,
            },
            secrets: SecretGrant {
                names: BTreeSet::new(),
            },
            process: ProcessGrant {
                programs: ["*".to_string()].into_iter().collect(),
                environment: ["PATH", "LANG", "LC_ALL", "TERM", "TMPDIR"]
                    .into_iter()
                    .map(str::to_string)
                    .collect(),
                inherit_stdio: false,
            },
            resource_limits: ResourceLimits::default(),
            sandbox: SandboxRequirement::NotRequired,
            expires_at: SystemTime::now() + Duration::from_secs(24 * 60 * 60),
        }
    }

    pub fn is_expired(&self) -> bool {
        SystemTime::now() >= self.expires_at
    }

    pub fn intersect(&self, child: &Self) -> Result<Self, GrantError> {
        if self.principal != child.principal {
            return Err(GrantError::PrincipalChanged);
        }
        let expires_at = self.expires_at.min(child.expires_at);
        let filesystem = FsGrant {
            read_roots: intersect_roots(&self.filesystem.read_roots, &child.filesystem.read_roots),
            write_roots: intersect_roots(
                &self.filesystem.write_roots,
                &child.filesystem.write_roots,
            ),
            max_file_bytes: self
                .filesystem
                .max_file_bytes
                .min(child.filesystem.max_file_bytes),
        };
        let process = ProcessGrant {
            programs: intersect_programs(&self.process.programs, &child.process.programs),
            environment: self
                .process
                .environment
                .intersection(&child.process.environment)
                .cloned()
                .collect(),
            inherit_stdio: self.process.inherit_stdio && child.process.inherit_stdio,
        };
        Ok(Self {
            principal: self.principal.clone(),
            filesystem,
            network: NetworkGrant {
                hosts: intersect_set(&self.network.hosts, &child.network.hosts),
                ports: self
                    .network
                    .ports
                    .intersection(&child.network.ports)
                    .copied()
                    .collect(),
                allow_private: self.network.allow_private && child.network.allow_private,
                max_redirects: self.network.max_redirects.min(child.network.max_redirects),
            },
            secrets: SecretGrant {
                names: self
                    .secrets
                    .names
                    .intersection(&child.secrets.names)
                    .cloned()
                    .collect(),
            },
            process,
            resource_limits: ResourceLimits {
                cpu_seconds: self
                    .resource_limits
                    .cpu_seconds
                    .min(child.resource_limits.cpu_seconds),
                address_space_bytes: self
                    .resource_limits
                    .address_space_bytes
                    .min(child.resource_limits.address_space_bytes),
                open_files: self
                    .resource_limits
                    .open_files
                    .min(child.resource_limits.open_files),
                processes: self
                    .resource_limits
                    .processes
                    .min(child.resource_limits.processes),
                file_bytes: self
                    .resource_limits
                    .file_bytes
                    .min(child.resource_limits.file_bytes),
            },
            sandbox: if self.sandbox == SandboxRequirement::Enforced
                || child.sandbox == SandboxRequirement::Enforced
            {
                SandboxRequirement::Enforced
            } else {
                SandboxRequirement::NotRequired
            },
            expires_at,
        })
    }

    pub fn permits_program(&self, program: &std::ffi::OsStr) -> bool {
        if self.is_expired() {
            return false;
        }
        let value = program.to_string_lossy();
        self.process.programs.contains("*")
            || self.process.programs.contains(value.as_ref())
            || std::path::Path::new(value.as_ref())
                .file_name()
                .is_some_and(|v| {
                    self.process
                        .programs
                        .contains(&v.to_string_lossy().into_owned())
                })
    }
}

fn intersect_set<T: Ord + Clone>(left: &BTreeSet<T>, right: &BTreeSet<T>) -> BTreeSet<T> {
    left.intersection(right).cloned().collect()
}

fn intersect_programs(left: &BTreeSet<String>, right: &BTreeSet<String>) -> BTreeSet<String> {
    if left.contains("*") {
        return right.clone();
    }
    if right.contains("*") {
        return left.clone();
    }
    left.intersection(right).cloned().collect()
}

fn intersect_roots(left: &[PathBuf], right: &[PathBuf]) -> Vec<PathBuf> {
    let mut result = BTreeSet::new();
    for a in left {
        for b in right {
            if a.starts_with(b) {
                result.insert(a.clone());
            } else if b.starts_with(a) {
                result.insert(b.clone());
            }
        }
    }
    result.into_iter().collect()
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum GrantError {
    PrincipalChanged,
    Expired,
    ProgramDenied(String),
    SandboxUnavailable(String),
    FilesystemDenied(String),
    NetworkDenied(String),
    SecretDenied(String),
    ResourceLimit(String),
}
impl std::fmt::Display for GrantError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::PrincipalChanged => f.write_str("child grant cannot change principal"),
            Self::Expired => f.write_str("execution grant expired"),
            Self::ProgramDenied(v) => write!(f, "process execution denied: {v}"),
            Self::SandboxUnavailable(v) => write!(f, "enforced sandbox unavailable: {v}"),
            Self::FilesystemDenied(v) => write!(f, "filesystem access denied: {v}"),
            Self::NetworkDenied(v) => write!(f, "network access denied: {v}"),
            Self::SecretDenied(v) => write!(f, "secret access denied: {v}"),
            Self::ResourceLimit(v) => write!(f, "resource limit rejected: {v}"),
        }
    }
}
impl std::error::Error for GrantError {}
