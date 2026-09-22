//! Running the JavaScript host that loads the extensions and answers what they
//! declare, in a real node process with the sources it was given.

use serde_json::{json, Value};
use std::path::Path;
use std::process::{Command, Stdio};
use std::sync::mpsc;
use std::time::Duration;
use std::io::Read;

use super::super::{ABI_VERSION, HOST};

pub(super) fn node_supports_typescript(node: &str) -> bool {
    Command::new(node)
        .args(["--experimental-strip-types", "--eval", ""])
        .output()
        .map(|output| output.status.success())
        .unwrap_or(false)
}

// These parameters are the extension host's invocation contract: mode,
// generation, env, sources, both authorization flags, and the borrowed
// operation context. Nothing in scope owns that set together.
#[allow(clippy::too_many_arguments)]
pub(super) fn run_host(
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
                artifact_dir.unwrap_or(cwd).to_path_buf(),
            ),
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
    let node = env::var("JEDEN_NODE").unwrap_or_else(|_| "node".into());
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
        envs.iter().any(|(_, value)| value.contains(".ts")) && node_supports_typescript(&node);
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
            "JEDEN_EXTENSION_ALLOW_WRITE",
            if allow_write { "1" } else { "0" },
        )
        .env(
            "JEDEN_EXTENSION_ALLOW_COMMAND",
            if allow_command { "1" } else { "0" },
        )
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
    let mut stdout = child.stdout.take().ok_or("extension host missing stdout")?;
    let mut stderr = child.stderr.take().ok_or("extension host missing stderr")?;
    let (progress_tx, progress_rx) = mpsc::channel();
    let stdout_progress = progress_tx.clone();
    let stdout_reader = std::thread::spawn(move || {
        let mut bytes = Vec::new();
        let mut buffer = [0u8; 8192];
        let mut total = 0u64;
        loop {
            let count = stdout.read(&mut buffer).unwrap_or_default();
            if count == 0 {
                break;
            }
            total = total.saturating_add(count as u64);
            let remaining = (MAX_DESCRIPTOR_BYTES + 1).saturating_sub(bytes.len());
            bytes.extend_from_slice(&buffer[..count.min(remaining)]);
            let _ = stdout_progress.send(("extension-stdout", count as u64, total));
        }
        bytes
    });
    let stderr_reader = std::thread::spawn(move || {
        let mut bytes = Vec::new();
        let mut buffer = [0u8; 8192];
        loop {
            let count = stderr.read(&mut buffer).unwrap_or_default();
            if count == 0 {
                break;
            }
            let remaining = (MAX_DESCRIPTOR_BYTES + 1).saturating_sub(bytes.len());
            bytes.extend_from_slice(&buffer[..count.min(remaining)]);
        }
        bytes
    });
    drop(progress_tx);
    let status = loop {
        while let Ok((stream, bytes, total_bytes)) = progress_rx.try_recv() {
            if let Some(context) = operation {
                context.progress(crate::tool_runtime::runtime_ops::OperationProgress {
                    stream,
                    bytes,
                    total_bytes,
                });
            }
        }
        if operation.is_some_and(|context| context.cancellation().is_cancelled()) {
            let _ = child.kill();
            let _ = child.wait();
            let _ = stdout_reader.join();
            let _ = stderr_reader.join();
            return Err("extension operation cancelled".into());
        }
        if let Some(status) = child.try_wait().map_err(|error| error.to_string())? {
            break status;
        }
        std::thread::sleep(Duration::from_millis(20));
    };
    let stdout = stdout_reader.join().unwrap_or_default();
    let stderr = stderr_reader.join().unwrap_or_default();
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
    if stdout.len() > MAX_DESCRIPTOR_BYTES || stderr.len() > MAX_DESCRIPTOR_BYTES {
        return Err("extension host output exceeded 2 MiB".into());
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
        return Err(value
            .get("error")
            .and_then(Value::as_str)
            .unwrap_or_else(|| stderr.trim())
            .to_string());
    }
    Ok(value)
}
