//! Running the JavaScript host that loads the extensions and answers what they
//! declare, in a real node process with the sources it was given.

use serde_json::Value;
use std::path::Path;
use std::process::{Command, Stdio};
use std::sync::mpsc;

use super::host_stream::{drained, node_supports_typescript, read_stderr, read_stdout, HostEvent};
use super::super::HOST;
use std::env;
use std::path::PathBuf;

// These parameters are the extension host's invocation contract: mode,
// generation, env, sources, both authorization flags, and the borrowed
// operation context. Nothing in scope owns that set together.
#[allow(clippy::too_many_arguments)]
pub(crate) fn run_host(
    cwd: &Path,
    mode: &str,
    generation: u64,
    envs: &[(&str, String)],
    source_paths: &[PathBuf],
    allow_write: bool,
    allow_command: bool,
    artifact_dir: Option<&Path>,
    operation: Option<&crate::tool_runtime::runtime_ops::OperationContext<'_>>,
) -> Result<Value, String> {
    let canonical_sources = source_paths
        .iter()
        .map(|path| {
            path.canonicalize()
                .map_err(|error| format!("extension source unavailable: {error}"))
        })
        .collect::<Result<Vec<_>, _>>()?;
    let owned_context;
    let secured = if let Some(context) = operation {
        crate::tool_runtime::runtime_ops::untrusted_child(
            context,
            format!("{}:extension:{mode}", context.operation_id()),
        )
        .map_err(|error| error.to_string())?
    } else {
        let mut grant = crate::tool_runtime::runtime_ops::ExecutionGrant::trusted_host(
            "extension-discovery",
            cwd.to_path_buf(),
        );
        grant
            .filesystem
            .read_roots
            .extend(canonical_sources.iter().cloned());
        grant.sandbox = crate::tool_runtime::runtime_ops::SandboxRequirement::Enforced;
        owned_context = crate::tool_runtime::runtime_ops::OperationContext::new(
            crate::tool_runtime::runtime_ops::CancellationToken::new(),
            crate::tool_runtime::runtime_ops::ArtifactSink::new(
                // Without an artifact directory, the host's artifacts are kept
                // in the directory it runs in.
                match artifact_dir {
                    Some(directory) => directory,
                    None => cwd,
                }
                .to_path_buf(),
            ),
            cwd.to_path_buf(),
        )
        .with_execution_grant(grant);
        crate::tool_runtime::runtime_ops::untrusted_child(
            &owned_context,
            format!("extension:{mode}"),
        )
        .map_err(|error| error.to_string())?
    };
    let grant = secured.execution_grant();
    if operation.is_some_and(|context| context.cancellation().is_cancelled()) {
        return Err("extension operation cancelled".into());
    }
    // Unset means `node` on PATH; an override that is not text is refused
    // rather than silently replaced by it.
    let node = match env::var("JEDEN_NODE") {
        Ok(node) => node,
        Err(env::VarError::NotPresent) => "node".to_string(),
        Err(error) => return Err(format!("JEDEN_NODE cannot be read: {error}")),
    };
    if !grant.permits_program(std::ffi::OsStr::new(&node)) {
        return Err(
            crate::tool_runtime::runtime_ops::GrantError::ProgramDenied(node.clone()).to_string(),
        );
    }
    let canonical_cwd = cwd
        .canonicalize()
        .map_err(|error| format!("extension cwd unavailable: {error}"))?;
    if !grant
        .filesystem
        .read_roots
        .iter()
        .any(|root| canonical_cwd.starts_with(root))
    {
        return Err(
            crate::tool_runtime::runtime_ops::GrantError::FilesystemDenied(format!(
                "extension cwd {} is outside grant",
                canonical_cwd.display()
            ))
            .to_string(),
        );
    }
    for source in &canonical_sources {
        if !grant
            .filesystem
            .read_roots
            .iter()
            .any(|root| source.starts_with(root))
        {
            return Err(
                crate::tool_runtime::runtime_ops::GrantError::FilesystemDenied(format!(
                    "extension source {} is outside grant",
                    source.display()
                ))
                .to_string(),
            );
        }
    }
    let enable_ts =
        envs.iter().any(|(_, value)| value.contains(".ts")) && node_supports_typescript(&node)?;
    let mut command = Command::new(&node);
    command.env_clear();
    for key in &grant.process.environment {
        if let Some(value) = env::var_os(key) {
            command.env(key, value);
        }
    }
    command.arg("--input-type=module");
    if enable_ts {
        command.arg("--experimental-strip-types");
    }
    command
        .args(["-e", HOST])
        .env("JEDEN_EXTENSION_MODE", mode)
        .env("JEDEN_EXTENSION_CWD", &canonical_cwd)
        .env("JEDEN_EXTENSION_GENERATION", generation.to_string())
        .env(
            "JEDEN_EXTENSION_ABI_VERSION",
            crate::hooks::extensions::ABI_VERSION.to_string(),
        )
        .env("JEDEN_EXTENSION_ALLOW_WRITE", allow_write.to_string())
        .env("JEDEN_EXTENSION_ALLOW_COMMAND", allow_command.to_string())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    if let Some(dir) = operation
        .map(|context| context.artifacts().root())
        .or(artifact_dir)
    {
        command.env("JEDEN_EXTENSION_ARTIFACT_DIR", dir);
    }
    for (key, value) in envs {
        command.env(key, value);
    }
    let mut child = command
        .spawn()
        .map_err(|error| format!("extension host failed to start: {error}"))?;
    let stdout = child.stdout.take().ok_or("extension host missing stdout")?;
    let stderr = child.stderr.take().ok_or("extension host missing stderr")?;
    let (events_tx, events_rx) = mpsc::channel();
    // Each stream is drained whole; a read that fails is a failure of the run,
    // never the end of the host's answer.
    let stdout_reader = read_stdout(stdout, events_tx.clone());
    let stderr_reader = read_stderr(stderr);
    if operation.is_some() {
        let wake = events_tx.clone();
        crate::tool_runtime::runtime_ops::wake_on_cancellation(move || {
            wake.send(HostEvent::Cancelled).is_ok()
        });
    }
    drop(events_tx);
    // The host writes its answer and closes stdout when it is done; its exit
    // is then awaited. A cancelled turn kills it instead.
    loop {
        if operation.is_some_and(|context| context.cancellation().is_cancelled()) {
            if let Err(error) = child.kill() {
                return Err(format!(
                    "extension operation cancelled; its host process {} could not be killed: {error}",
                    child.id()
                ));
            }
            // The readers end when the killed host's pipes close; what they
            // read belongs to a cancelled run and is not waited for.
            let process = child.id();
            if let Err(error) = stado_wait::blocking(
                stado_wait::Kind::Process,
                format!("killed extension host process {process} to end"),
                &node,
                || child.wait(),
            ) {
                return Err(format!(
                    "extension operation cancelled; its killed host process {process} could not be reaped: {error}"
                ));
            }
            return Err("extension operation cancelled".into());
        }
        match events_rx.recv() {
            Ok(HostEvent::Progress(bytes, total_bytes)) => {
                if let Some(context) = operation {
                    context.progress(crate::tool_runtime::runtime_ops::OperationProgress {
                        stream: "extension-stdout",
                        bytes,
                        total_bytes,
                    });
                }
            }
            Ok(HostEvent::Cancelled) => {}
            Ok(HostEvent::StdoutClosed) => break,
            // Every reader has ended and dropped its sender: nothing more comes.
            Err(_readers_ended) => break,
        }
    }
    let process = child.id();
    let status = stado_wait::blocking(
        stado_wait::Kind::Process,
        format!("extension host process {process} to end"),
        &node,
        || child.wait(),
    )
    .map_err(|error| format!("extension host process {process} could not be awaited: {error}"))?;
    let stdout = drained(stdout_reader, "stdout")?;
    let stderr = drained(stderr_reader, "stderr")?;
    if let Some(context) = operation {
        let mut output = crate::tool_runtime::runtime_ops::BoundedOutput::new(
            "extension-host",
            context.output_limits(),
            context.artifacts().clone(),
        );
        output
            .write_chunk(&stdout)
            .map_err(|error| error.to_string())?;
        output
            .write_chunk(&stderr)
            .map_err(|error| error.to_string())?;
        output.finish().map_err(|error| error.to_string())?;
    }
    let stdout = String::from_utf8_lossy(&stdout);
    let stderr = String::from_utf8_lossy(&stderr);
    let line = stdout
        .lines()
        .rev()
        .find_map(|line| line.strip_prefix("JEDEN_EXTENSION\t"))
        .ok_or_else(|| format!("extension host returned no protocol frame: {stderr}"))?;
    let value: Value = serde_json::from_str(line)
        .map_err(|error| format!("invalid extension host frame: {error}"))?;
    if !status.success() || value.get("ok").and_then(Value::as_bool) == Some(false) {
        return Err(match value.get("error").and_then(Value::as_str) {
            Some(said) => said.to_string(),
            None => format!(
                "the extension host failed ({status}) without saying why; its stderr: {}",
                stderr.trim()
            ),
        });
    }
    Ok(value)
}
