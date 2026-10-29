//! Rebuilding Jeden from the source workspace it runs in and relaunching the
//! session in the new build: cargo builds the binary, the build is signed
//! with the identity the running one carried, and the candidate must answer
//! `--version` as Jeden before the session moves to it.

use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::process::Command;

use serde_json::Value;

use crate::Args;

mod checks;
#[cfg(target_os = "macos")]
mod signing;

use checks::{command_failure, ensure_jeden_package, verify_candidate};

#[derive(Debug)]
pub(crate) struct RelaunchPlan {
    executable: PathBuf,
    arguments: Vec<OsString>,
}

/// The cargo that runs this build: the one cargo names in `CARGO` when it
/// started Jeden, else `cargo` on the PATH.
fn cargo() -> OsString {
    match std::env::var_os("CARGO") {
        Some(cargo) => cargo,
        None => OsString::from("cargo"),
    }
}

pub(crate) fn prepare(args: &Args, session_path: &Path) -> Result<RelaunchPlan, String> {
    let manifest = args.cwd.join("Cargo.toml");
    if !manifest.is_file() {
        return Err(format!(
            "self-rebuild requires the Jeden source workspace; {} is missing",
            manifest.display()
        ));
    }
    let cargo = cargo();
    let mut metadata = Command::new(&cargo);
    metadata
        .args([
            "metadata",
            "--format-version",
            "1",
            "--no-deps",
            "--manifest-path",
        ])
        .arg(&manifest);
    let metadata = stado_wait::output(&mut metadata)
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
    let signer = signing::Signer::for_running_build()?;

    let mut build = Command::new(&cargo);
    build
        .args(["build", "--release", "--manifest-path"])
        .arg(&manifest)
        .args(["--bin", "jeden"]);
    let status = stado_wait::status(&mut build)
        .map_err(|error| format!("failed to start cargo build: {error}"))?;
    if !status.success() {
        return Err(format!("cargo build failed with status {status}"));
    }

    let executable = target_dir
        .join("release")
        .join(format!("jeden{}", std::env::consts::EXE_SUFFIX));
    #[cfg(target_os = "macos")]
    signer.sign(&executable)?;
    verify_candidate(&executable)?;
    Ok(RelaunchPlan {
        executable,
        arguments: relaunch_arguments(args, session_path),
    })
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
