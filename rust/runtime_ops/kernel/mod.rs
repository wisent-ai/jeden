//! Persistent interpreter kernels: which one a scope is talking to, and what
//! happens to it when the scope goes away.

use super::{ArtifactSink, CancellationToken, OperationContext, OutputCapture};
use std::collections::HashMap;
use std::ffi::OsStr;
use std::path::{Path, PathBuf};
use std::sync::{LazyLock, Mutex};
use std::time::Duration;

mod bootstrap;
mod process;

use crate::tool_runtime::runtime_ops::security::ExecutionGrant;
use process::KernelProcess;

const FRAME_LIMIT: usize = 64 * 1024;
const POLL: Duration = Duration::from_millis(10);

static KERNELS: LazyLock<Mutex<HashMap<KernelKey, KernelProcess>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum KernelLanguage {
    Python,
    JavaScript,
}

impl KernelLanguage {
    pub fn parse(value: &str) -> Result<Self, String> {
        match value {
            "python" | "py" => Ok(Self::Python),
            "javascript" | "js" | "node" => Ok(Self::JavaScript),
            _ => Err(format!("unsupported kernel language: {value}")),
        }
    }
    fn label(self) -> &'static str {
        match self {
            Self::Python => "python",
            Self::JavaScript => "javascript",
        }
    }
}

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
struct KernelKey {
    scope: PathBuf,
    cwd: PathBuf,
    language: KernelLanguage,
}

pub struct KernelResult {
    pub ok: bool,
    pub cancelled: bool,
    pub reset: bool,
    pub stdout: OutputCapture,
    pub stderr: OutputCapture,
    pub display: OutputCapture,
    pub display_mime: Option<String>,
    pub error: Option<String>,
}

pub fn evaluate(
    context: &OperationContext<'_>,
    scope: &Path,
    cwd: &Path,
    language: KernelLanguage,
    code: &str,
    reset: bool,
) -> Result<KernelResult, String> {
    let child = super::untrusted_child(
        context,
        format!("{}:kernel:{}", context.operation_id(), language.label()),
    )
    .map_err(|error| error.to_string())?;
    let grant = child.execution_grant();
    let program = match language {
        KernelLanguage::Python => OsStr::new("python3"),
        KernelLanguage::JavaScript => OsStr::new("node"),
    };
    if !grant.permits_program(program) {
        return Err(
            super::GrantError::ProgramDenied(program.to_string_lossy().into_owned()).to_string(),
        );
    }
    let canonical_cwd = cwd
        .canonicalize()
        .map_err(|error| format!("kernel cwd unavailable: {error}"))?;
    if !grant
        .filesystem
        .read_roots
        .iter()
        .any(|root| canonical_cwd.starts_with(root))
    {
        return Err(super::GrantError::FilesystemDenied(format!(
            "kernel cwd {} is outside grant",
            canonical_cwd.display()
        ))
        .to_string());
    }
    let key = KernelKey {
        scope: scope.to_path_buf(),
        cwd: canonical_cwd.clone(),
        language,
    };
    let mut kernels = KERNELS
        .lock()
        .map_err(|_| "kernel registry lock poisoned")?;
    if reset {
        if let Some(mut old) = kernels.remove(&key) {
            old.terminate();
        }
    }
    let mut kernel = if let Some(mut existing) = kernels.remove(&key) {
        if existing.alive() {
            existing
        } else {
            existing.terminate();
            KernelProcess::spawn(language, &canonical_cwd, grant)?
        }
    } else {
        KernelProcess::spawn(language, &canonical_cwd, grant)?
    };
    let outcome = kernel.evaluate(context, code, reset);
    match outcome {
        Ok((result, healthy)) => {
            if healthy {
                kernels.insert(key, kernel);
            } else {
                kernel.terminate();
            }
            Ok(result)
        }
        Err(error) => {
            kernel.terminate();
            Err(error)
        }
    }
}

pub fn probe(language: KernelLanguage, cwd: &Path) -> Result<(), String> {
    let mut kernel = KernelProcess::spawn(
        language,
        cwd,
        OperationContext::new(
            CancellationToken::new(),
            ArtifactSink::new(std::env::temp_dir()),
        )
        .execution_grant(),
    )?;
    let artifacts = std::env::temp_dir().join("jeden-kernel-probe-artifacts");
    let context = OperationContext::new(CancellationToken::new(), ArtifactSink::new(artifacts));
    let result = kernel.evaluate(&context, "1", true);
    kernel.terminate();
    let (result, _) = result?;
    if result.ok {
        Ok(())
    } else {
        Err(result
            .error
            .unwrap_or_else(|| format!("{} kernel probe failed", language.label())))
    }
}

pub fn teardown_scope(scope: &Path) {
    if let Ok(mut kernels) = KERNELS.lock() {
        let keys: Vec<_> = kernels
            .keys()
            .filter(|key| key.scope == scope)
            .cloned()
            .collect();
        for key in keys {
            if let Some(mut kernel) = kernels.remove(&key) {
                kernel.terminate();
            }
        }
    }
}
