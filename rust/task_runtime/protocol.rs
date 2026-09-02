use super::cas::Digest;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};

pub const WORKER_PROTOCOL_MAJOR: u16 = 1;
pub const WORKER_PROTOCOL_MINOR: u16 = 0;

#[derive(Clone, Copy, Debug, Deserialize, Eq, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProtocolVersion {
    pub major: u16,
    pub minor: u16,
}
impl ProtocolVersion {
    pub const V1: Self = Self {
        major: WORKER_PROTOCOL_MAJOR,
        minor: WORKER_PROTOCOL_MINOR,
    };
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct VersionRange {
    pub minimum: ProtocolVersion,
    pub maximum: ProtocolVersion,
}
impl Default for VersionRange {
    fn default() -> Self {
        Self {
            minimum: ProtocolVersion::V1,
            maximum: ProtocolVersion::V1,
        }
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkerHello {
    pub worker_id: String,
    pub versions: VersionRange,
    pub descriptor: WorkerDescriptor,
    pub incarnation: u64,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct NegotiatedHello {
    pub worker_id: String,
    pub version: ProtocolVersion,
    pub coordinator_epoch: u64,
}

pub fn negotiate_version(range: &VersionRange) -> Result<ProtocolVersion, ProtocolError> {
    if range.minimum > range.maximum {
        return Err(ProtocolError::Invalid(
            "worker version range is inverted".into(),
        ));
    }
    if range.minimum.major > WORKER_PROTOCOL_MAJOR || range.maximum.major < WORKER_PROTOCOL_MAJOR {
        return Err(ProtocolError::UnsupportedVersion {
            minimum: range.minimum,
            maximum: range.maximum,
        });
    }
    let minimum_minor = if range.minimum.major == WORKER_PROTOCOL_MAJOR {
        range.minimum.minor
    } else {
        0
    };
    let maximum_minor = if range.maximum.major == WORKER_PROTOCOL_MAJOR {
        range.maximum.minor
    } else {
        u16::MAX
    };
    if minimum_minor > WORKER_PROTOCOL_MINOR || maximum_minor < minimum_minor {
        return Err(ProtocolError::UnsupportedVersion {
            minimum: range.minimum,
            maximum: range.maximum,
        });
    }
    Ok(ProtocolVersion {
        major: WORKER_PROTOCOL_MAJOR,
        minor: WORKER_PROTOCOL_MINOR,
    })
}

#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Resources {
    pub cpu_millis: u64,
    pub memory_bytes: u64,
    pub disk_bytes: u64,
}
impl Resources {
    pub fn fits(&self, required: &Self) -> bool {
        self.cpu_millis >= required.cpu_millis
            && self.memory_bytes >= required.memory_bytes
            && self.disk_bytes >= required.disk_bytes
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkerDescriptor {
    pub os: String,
    pub arch: String,
    #[serde(default)]
    pub capabilities: BTreeSet<String>,
    #[serde(default)]
    pub sandbox_profiles: BTreeSet<String>,
    #[serde(default)]
    pub trust_zones: BTreeSet<String>,
    #[serde(default)]
    pub residencies: BTreeSet<String>,
    #[serde(default)]
    pub resources: Resources,
    #[serde(default)]
    pub cas_objects: BTreeSet<Digest>,
    #[serde(default)]
    pub labels: BTreeMap<String, String>,
    #[serde(default)]
    pub max_parallel: u32,
}

#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", default)]
pub struct PlacementConstraints {
    pub os: Option<String>,
    pub arch: Option<String>,
    pub capabilities: BTreeSet<String>,
    pub sandbox_profile: Option<String>,
    pub trust_zone: Option<String>,
    pub residency: Option<String>,
    pub resources: Resources,
    pub labels: BTreeMap<String, String>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Job {
    pub id: String,
    pub input_root: Digest,
    pub constraints: PlacementConstraints,
    #[serde(default)]
    pub payload: Vec<u8>,
    pub created_at: u64,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum AttemptPhase {
    Offered,
    Accepted,
    Materializing,
    Running,
    Uploading,
    CommitReady,
    Succeeded,
    Failed,
    Cancelled,
}
impl AttemptPhase {
    pub fn terminal(self) -> bool {
        matches!(self, Self::Succeeded | Self::Failed | Self::Cancelled)
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Attempt {
    pub job_id: String,
    pub number: u32,
    pub worker_id: String,
    pub fencing_token: u64,
    pub phase: AttemptPhase,
    pub started_at: u64,
    pub updated_at: u64,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Lease {
    pub job_id: String,
    pub attempt: u32,
    pub worker_id: String,
    pub fencing_token: u64,
    pub expires_at: u64,
    pub heartbeat_at: u64,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Worker {
    pub hello: WorkerHello,
    pub negotiated: ProtocolVersion,
    pub last_heartbeat: u64,
    pub running: u32,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum JobPhase {
    Pending,
    Assigned,
    Running,
    Cancelling,
    Cancelled,
    Succeeded,
    Failed,
}
impl JobPhase {
    pub fn terminal(self) -> bool {
        matches!(self, Self::Cancelled | Self::Succeeded | Self::Failed)
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkerEvent {
    pub job_id: String,
    pub attempt: u32,
    pub fencing_token: u64,
    pub sequence: u64,
    pub phase: AttemptPhase,
    #[serde(default)]
    pub detail: String,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkOffer {
    pub protocol: ProtocolVersion,
    pub job: Job,
    pub attempt: u32,
    pub fencing_token: u64,
    pub lease_expires_at: u64,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CommitRequest {
    pub job_id: String,
    pub attempt: u32,
    pub fencing_token: u64,
    pub output_root: Digest,
    #[serde(default)]
    pub result: Vec<u8>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct JobOutcome {
    pub job_id: String,
    pub attempt: u32,
    pub output_root: Digest,
    #[serde(default)]
    pub result: Vec<u8>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ProtocolError {
    UnsupportedVersion {
        minimum: ProtocolVersion,
        maximum: ProtocolVersion,
    },
    Invalid(String),
    NotFound(String),
    NoPlacement(String),
    LeaseLost(String),
    StaleFence {
        expected: u64,
        actual: u64,
    },
    Conflict(String),
    Cancelled(String),
    Storage(String),
    Transport(String),
}
impl std::fmt::Display for ProtocolError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::UnsupportedVersion { minimum, maximum } => write!(
                f,
                "unsupported worker protocol range {}.{}..={}.{}",
                minimum.major, minimum.minor, maximum.major, maximum.minor
            ),
            Self::Invalid(v)
            | Self::NotFound(v)
            | Self::NoPlacement(v)
            | Self::LeaseLost(v)
            | Self::Conflict(v)
            | Self::Cancelled(v)
            | Self::Storage(v)
            | Self::Transport(v) => f.write_str(v),
            Self::StaleFence { expected, actual } => write!(
                f,
                "stale fencing token {actual}; current token is {expected}"
            ),
        }
    }
}
impl std::error::Error for ProtocolError {}
