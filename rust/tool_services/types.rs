use crate::tool_runtime::runtime_ops::{BoundedOutput, OperationContext, OperationProgress};
use serde::Serialize;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::fmt;
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

const MAX_MEDIA_BYTES: usize = 32 * 1024 * 1024;
static ARTIFACT_ID: AtomicU64 = AtomicU64::new(1);

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum HealthStatus {
    Healthy,
    Unavailable,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct HealthDescriptor {
    pub service: &'static str,
    pub status: HealthStatus,
    pub backend: Option<String>,
    pub detail: String,
}

impl HealthDescriptor {
    pub fn healthy(service: &'static str, backend: impl Into<String>) -> Self {
        Self {
            service,
            status: HealthStatus::Healthy,
            backend: Some(backend.into()),
            detail: "configured and discoverable".into(),
        }
    }
    pub fn unavailable(service: &'static str, detail: impl Into<String>) -> Self {
        Self {
            service,
            status: HealthStatus::Unavailable,
            backend: None,
            detail: detail.into(),
        }
    }
    pub fn available(&self) -> bool {
        self.status != HealthStatus::Unavailable
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ServiceError {
    Unavailable {
        service: &'static str,
        detail: String,
    },
    InvalidInput(String),
    PermissionDenied(String),
    Cancelled,
    DeadlineExceeded,
    Backend {
        service: &'static str,
        detail: String,
    },
    Protocol {
        service: &'static str,
        detail: String,
    },
    OutputLimit {
        limit: usize,
    },
    Io(String),
}

impl fmt::Display for ServiceError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Unavailable { service, detail } => write!(f, "{service} unavailable: {detail}"),
            Self::InvalidInput(v) => write!(f, "invalid input: {v}"),
            Self::PermissionDenied(v) => write!(f, "permission denied: {v}"),
            Self::Cancelled => f.write_str("operation cancelled"),
            Self::DeadlineExceeded => f.write_str("operation deadline exceeded"),
            Self::Backend { service, detail } => write!(f, "{service} backend failed: {detail}"),
            Self::Protocol { service, detail } => write!(f, "{service} protocol error: {detail}"),
            Self::OutputLimit { limit } => write!(f, "output exceeded {limit} bytes"),
            Self::Io(v) => write!(f, "I/O error: {v}"),
        }
    }
}
impl std::error::Error for ServiceError {}
impl From<std::io::Error> for ServiceError {
    fn from(value: std::io::Error) -> Self {
        Self::Io(value.to_string())
    }
}

pub type ServiceResult<T> = Result<T, ServiceError>;

pub fn check_operation(context: &OperationContext<'_>) -> ServiceResult<()> {
    if context.cancellation().is_cancelled() {
        return Err(ServiceError::Cancelled);
    }
    if context
        .deadline()
        .is_some_and(|deadline| std::time::Instant::now() >= deadline)
    {
        return Err(ServiceError::DeadlineExceeded);
    }
    Ok(())
}

pub fn bounded_json(
    context: &OperationContext<'_>,
    service: &'static str,
    value: &Value,
) -> ServiceResult<Value> {
    check_operation(context)?;
    let bytes = serde_json::to_vec(value).map_err(|e| ServiceError::Protocol {
        service,
        detail: e.to_string(),
    })?;
    let mut output = BoundedOutput::new(
        service,
        context.output_limits(),
        context.artifacts().clone(),
    );
    output.write_chunk(&bytes)?;
    context.progress(OperationProgress {
        stream: service,
        bytes: bytes.len() as u64,
        total_bytes: bytes.len() as u64,
    });
    let capture = output.finish()?;
    if !capture.truncated {
        return Ok(value.clone());
    }
    Ok(json!({
        "ok": true,
        "truncated": true,
        "preview": capture.text,
        "totalBytes": capture.total_bytes,
        "sha256": capture.sha256,
        "artifact": capture.artifact.map(|p| p.display().to_string())
    }))
}

pub fn write_media_artifact(
    context: &OperationContext<'_>,
    service: &'static str,
    extension: &str,
    bytes: &[u8],
) -> ServiceResult<Value> {
    check_operation(context)?;
    if bytes.len() > MAX_MEDIA_BYTES {
        return Err(ServiceError::OutputLimit {
            limit: MAX_MEDIA_BYTES,
        });
    }
    let extension = extension.trim_start_matches('.');
    if extension.is_empty() || !extension.bytes().all(|b| b.is_ascii_alphanumeric()) {
        return Err(ServiceError::InvalidInput(
            "invalid artifact extension".into(),
        ));
    }
    fs::create_dir_all(context.artifacts().root())?;
    let path = loop {
        let id = ARTIFACT_ID.fetch_add(1, Ordering::Relaxed);
        let candidate = context
            .artifacts()
            .root()
            .join(format!("{service}-{}-{id}.{extension}", std::process::id()));
        match OpenOptions::new()
            .create_new(true)
            .write(true)
            .open(&candidate)
        {
            Ok(mut file) => {
                file.write_all(bytes)?;
                file.sync_all()?;
                break candidate;
            }
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(error) => return Err(error.into()),
        }
    };
    context.progress(OperationProgress {
        stream: service,
        bytes: bytes.len() as u64,
        total_bytes: bytes.len() as u64,
    });
    Ok(
        json!({ "artifact": path.display().to_string(), "bytes": bytes.len(), "sha256": hex::encode(Sha256::digest(bytes)) }),
    )
}

pub fn nonempty(value: Option<&Value>, key: &str) -> ServiceResult<String> {
    value
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|v| !v.is_empty())
        .map(str::to_owned)
        .ok_or_else(|| ServiceError::InvalidInput(format!("{key} is required")))
}

pub fn command_exists(program: &str) -> bool {
    let path = Path::new(program);
    if path.components().count() > 1 {
        return path.is_file();
    }
    std::env::var_os("PATH")
        .into_iter()
        .flat_map(|paths| std::env::split_paths(&paths).collect::<Vec<_>>())
        .any(|directory| directory.join(program).is_file())
}

pub fn config_path(cwd: &Path) -> Vec<PathBuf> {
    let mut paths = Vec::new();
    if let Some(home) = std::env::var_os("HOME") {
        paths.push(PathBuf::from(home).join(".jeden/config.json"));
    }
    paths.push(cwd.join(".jeden/config.json"));
    paths
}
