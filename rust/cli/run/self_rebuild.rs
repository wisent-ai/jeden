use serde_json::Value;
use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::process::Command;

use crate::Args;

#[derive(Debug)]
pub(crate) struct RelaunchPlan {
    executable: PathBuf,
    arguments: Vec<OsString>,
}

pub(crate) fn prepare(args: &Args, session_path: &Path) -> Result<RelaunchPlan, String> {
    let manifest = args.cwd.join("Cargo.toml");
    if !manifest.is_file() {
        return Err(format!(
            "self-rebuild requires the Jeden source workspace; {} is missing",
            manifest.display()
        ));
    }

    let cargo = std::env::var_os("CARGO").unwrap_or_else(|| OsString::from("cargo"));
    let metadata = Command::new(&cargo)
        .args([
            "metadata",
            "--format-version",
            "1",
            "--no-deps",
            "--manifest-path",
        ])
        .arg(&manifest)
        .output()
        .map_err(|error| format!("failed to start cargo metadata: {error}"))?;
    if !metadata.status.success() {
        return Err(command_failure("cargo metadata", &metadata.stderr));
    }
    let metadata: Value = serde_json::from_slice(&metadata.stdout)
        .map_err(|error| format!("cargo metadata returned invalid JSON: {error}"))?;
    ensure_jeden_package(&metadata, &manifest)?;
    let target_dir = metadata
        .get("target_directory")
        .and_then(Value::as_str)
        .map(PathBuf::from)
        .ok_or("cargo metadata omitted target_directory")?;
    #[cfg(target_os = "macos")]
    let previous_signature = {
        let previous = std::env::current_exe()
            .map_err(|error| format!("cannot identify the running Jeden executable: {error}"))?;
        let report = Command::new("wisent-products")
            .args(["signing", "inspect", "--json"])
            .arg(previous)
            .output()
            .map_err(|error| format!("cannot inspect the running code identity: {error}"))?;
        let reports: Vec<Value> = serde_json::from_slice(&report.stdout)
            .map_err(|error| format!("cannot read the running code identity: {error}"))?;
        reports.into_iter().find(|row| row["state"] == "stable")
    };

    let status = Command::new(&cargo)
        .args(["build", "--release", "--manifest-path"])
        .arg(&manifest)
        .args(["--bin", "jeden"])
        .status()
        .map_err(|error| format!("failed to start cargo build: {error}"))?;
    if !status.success() {
        return Err(format!("cargo build failed with status {status}"));
    }

    let executable = target_dir
        .join("release")
        .join(format!("jeden{}", std::env::consts::EXE_SUFFIX));
    #[cfg(target_os = "macos")]
    {
        let mut signer = Command::new("wisent-products");
        signer.args(["signing", "sign"]);
        if let Some(previous) = &previous_signature {
            signer.args([
                "--identifier",
                previous["identifier"].as_str().ok_or("signing report omitted identifier")?,
                "--identity",
                previous["authority"].as_str().ok_or("signing report omitted authority")?,
            ]);
        } else {
            signer.args(["--product", "jeden"]);
        }
        let output = signer.arg(&executable).output()
            .map_err(|error| format!("cannot start stable macOS signing: {error}"))?;
        if !output.status.success() {
            return Err(command_failure("stable macOS signing", &output.stderr));
        }
        if let Some(previous) = previous_signature {
            let requirement = previous["requirement"].as_str().ok_or("signing report omitted requirement")?;
            let proof = Command::new("/usr/bin/codesign")
                .args(["--verify", "--strict", "-R"])
                .arg(format!("={requirement}"))
                .arg(&executable)
                .output()
                .map_err(|error| format!("cannot verify update identity: {error}"))?;
            if !proof.status.success() {
                return Err(command_failure("update changed macOS code identity", &proof.stderr));
            }
        }
    }
    verify_candidate(&executable)?;
    Ok(RelaunchPlan {
        executable,
        arguments: relaunch_arguments(args, session_path),
    })
}

fn ensure_jeden_package(metadata: &Value, manifest: &Path) -> Result<(), String> {
    let expected = manifest
        .canonicalize()
        .map_err(|error| format!("cannot resolve {}: {error}", manifest.display()))?;
    let found = metadata
        .get("packages")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .any(|package| {
            package.get("name").and_then(Value::as_str) == Some("jeden")
                && package
                    .get("manifest_path")
                    .and_then(Value::as_str)
                    .and_then(|path| Path::new(path).canonicalize().ok())
                    .as_deref()
                    == Some(expected.as_path())
                && package
                    .get("targets")
                    .and_then(Value::as_array)
                    .into_iter()
                    .flatten()
                    .any(|target| {
                        target.get("name").and_then(Value::as_str) == Some("jeden")
                            && target
                                .get("kind")
                                .and_then(Value::as_array)
                                .is_some_and(|kinds| kinds.iter().any(|kind| kind == "bin"))
                    })
        });
    if found {
        Ok(())
    } else {
        Err(format!(
            "{} is not the Jeden source manifest",
            manifest.display()
        ))
    }
}

fn verify_candidate(executable: &Path) -> Result<(), String> {
    let output = Command::new(executable)
        .arg("--version")
        .output()
        .map_err(|error| {
            format!(
                "rebuilt executable {} could not start: {error}",
                executable.display()
            )
        })?;
    if !output.status.success() {
        return Err(command_failure(
            &format!("{} --version", executable.display()),
            &output.stderr,
        ));
    }
    let version = String::from_utf8_lossy(&output.stdout);
    if !version.trim().starts_with("jeden ") {
        return Err(format!(
            "rebuilt executable returned an unexpected version response: {}",
            version.trim()
        ));
    }
    Ok(())
}

fn relaunch_arguments(args: &Args, session_path: &Path) -> Vec<OsString> {
    let mut arguments = vec![OsString::from("--cwd"), args.cwd.as_os_str().to_owned()];
    if let Some(model) = &args.model {
        arguments.extend([OsString::from("--model"), OsString::from(model)]);
    }
    if let Some(max_tokens) = args.max_tokens {
        arguments.extend([
            OsString::from("--max-tokens"),
            OsString::from(max_tokens.to_string()),
        ]);
    }
    if let Some(max_steps) = args.max_steps {
        arguments.extend([
            OsString::from("--max-steps"),
            OsString::from(max_steps.to_string()),
        ]);
    }
    if args.yolo {
        arguments.push(OsString::from("--yolo"));
    } else {
        if args.allow_write {
            arguments.push(OsString::from("--allow-write"));
        }
        if args.allow_command {
            arguments.push(OsString::from("--allow-command"));
        }
    }
    arguments.extend([
        OsString::from("--resume-session"),
        session_path.as_os_str().to_owned(),
    ]);
    arguments
}

pub(crate) fn execute(plan: RelaunchPlan) -> Result<(), String> {
    let mut command = Command::new(&plan.executable);
    command.args(&plan.arguments);
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        let error = command.exec();
        Err(format!(
            "failed to replace the running process with {}: {error}",
            plan.executable.display()
        ))
    }
    #[cfg(not(unix))]
    {
        command.spawn().map_err(|error| {
            format!(
                "failed to start rebuilt executable {}: {error}",
                plan.executable.display()
            )
        })?;
        Ok(())
    }
}

fn command_failure(command: &str, stderr: &[u8]) -> String {
    let detail = String::from_utf8_lossy(stderr).trim().to_string();
    if detail.is_empty() {
        format!("{command} failed")
    } else {
        format!("{command} failed: {detail}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn relaunch_preserves_runtime_permissions_and_session() {
        let args = Args {
            cwd: PathBuf::from("/workspace/jeden"),
            model: Some("provider/model".into()),
            max_tokens: Some(4096),
            max_steps: Some(23),
            allow_write: true,
            allow_command: true,
            ..Default::default()
        };
        let arguments = relaunch_arguments(&args, Path::new("/sessions/current"));
        let arguments = arguments
            .iter()
            .map(|argument| argument.to_string_lossy())
            .collect::<Vec<_>>();
        assert_eq!(
            arguments,
            [
                "--cwd",
                "/workspace/jeden",
                "--model",
                "provider/model",
                "--max-tokens",
                "4096",
                "--max-steps",
                "23",
                "--allow-write",
                "--allow-command",
                "--resume-session",
                "/sessions/current",
            ]
        );
    }

    #[test]
    fn yolo_does_not_duplicate_permission_flags() {
        let args = Args {
            cwd: PathBuf::from("/workspace/jeden"),
            allow_write: true,
            allow_command: true,
            yolo: true,
            ..Default::default()
        };
        let arguments = relaunch_arguments(&args, Path::new("/sessions/current"));
        assert!(arguments.iter().any(|argument| argument == "--yolo"));
        assert!(!arguments.iter().any(|argument| argument == "--allow-write"));
        assert!(!arguments
            .iter()
            .any(|argument| argument == "--allow-command"));
    }
}
