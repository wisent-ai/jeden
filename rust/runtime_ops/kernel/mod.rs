//! Persistent interpreter kernels: which one a scope is talking to, and what
//! happens to it when the scope goes away.

use super::{ArtifactSink, CancellationToken, OperationContext, OutputCapture};
use std::ffi::OsStr;
use std::path::{Path, PathBuf};
use std::sync::Arc;

mod bootstrap;
mod frame;
mod output;
mod process;

mod control;
mod launch;
mod registry;
use crate::tool_runtime::runtime_ops::security::ExecutionGrant;
use process::KernelProcess;
use super::session::PersistentProcess;


#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum KernelLanguage {
    Python,
    JavaScript,
}

impl KernelLanguage {
    /// The language `value` names, by the one label each language has — the
    /// names eval_session's input schema lists. The extra spellings `py`,
    /// `js` and `node` were outside that schema.
    pub fn parse(value: &str) -> Result<Self, String> {
        [Self::Python, Self::JavaScript]
            .into_iter()
            .find(|language| language.label() == value)
            .ok_or_else(|| format!("unsupported kernel language: {value}"))
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
        cwd: canonical_cwd,
        language,
    };
    let slot = registry::slot(key)?;
    let mut owned = slot.acquire(context)?;
    registry::check_current(&slot)?;
    if reset {
        if let Some(mut old) = owned.take() { old.retire(true); }
    }
    if let Some(existing) = owned.as_mut() {
        match existing.alive() {
            Ok(true) => {}
            Ok(false) => {
                let mut old = owned.take().expect("checked kernel disappeared while its lease was held");
                old.retire(false);
            }
            Err(cause) => {
                let mut old = owned.take().expect("unreadable kernel disappeared while its lease was held");
                old.retire(false);
                return Err(cause);
            }
        }
    }
    if owned.is_none() {
        let mut kernel = KernelProcess::spawn(language, &slot.key.cwd, grant)?;
        kernel.registered = Some(Arc::clone(&slot.key));
        registry::register(&slot, &kernel.control)?;
        *owned = Some(kernel);
    }
    let kernel = owned.as_mut().expect("initialized kernel disappeared while its lease was held");
    match kernel.evaluate(context, code, reset) {
        Ok((result, true)) => Ok(result),
        Ok((result, false)) => {
            let mut kernel = owned.take().expect("executed kernel disappeared while its lease was held");
            kernel.retire(result.cancelled);
            Ok(result)
        }
        Err(cause) => {
            let mut kernel = owned.take().expect("failed kernel disappeared while its lease was held");
            kernel.retire(context.cancellation().is_cancelled());
            Err(cause)
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
            cwd.to_path_buf(),
        )
        .execution_grant(),
    )?;
    let artifacts = std::env::temp_dir().join("jeden-kernel-probe-artifacts");
    let context = OperationContext::new(
        CancellationToken::new(),
        ArtifactSink::new(artifacts),
        cwd.to_path_buf(),
    );
    // An empty list is an expression every kernel language evaluates the
    // same way, so its success shows the kernel runs code at all.
    let result = kernel.evaluate(&context, "[]", true);
    kernel.retire(false);
    let (result, _) = result?;
    if result.ok {
        return Ok(());
    }
    match result.error {
        Some(error) => Err(error),
        None => Err(format!(
            "the {} kernel did not evaluate its probe and reported no error",
            language.label()
        )),
    }
}

pub fn teardown_scope(scope: &Path) {
    registry::teardown(scope);
}
