use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::{env, fs, process::Command};

fn required(name: &str) -> Result<String, String> {
    let value = env::var(name).map_err(|cause| format!("{name} is required: {cause}"))?;
    if value.trim().is_empty() { return Err(format!("{name} must not be empty")); }
    Ok(value)
}
fn git(report: &mut Value, args: &[&str]) -> Result<String, String> {
    let output = stado_wait::output(Command::new("git").current_dir(env!("CARGO_MANIFEST_DIR")).args(args))
        .map_err(|cause| format!("git {args:?}: {cause}"))?;
    report["commands"].as_array_mut().ok_or("report.commands is not an array")?
        .push(json!({"program":"git", "args":args, "exit":output.status.code(),
            "stdout":String::from_utf8_lossy(&output.stdout), "stderr":String::from_utf8_lossy(&output.stderr)}));
    if !output.status.success() {
        return Err(format!("git {args:?} failed {}: {}", output.status, String::from_utf8_lossy(&output.stderr)));
    }
    String::from_utf8(output.stdout).map(|value| value.trim().to_owned())
        .map_err(|cause| format!("git {args:?} output is not UTF-8: {cause}"))
}

/// Bind a native SDK journey to its clean checkout and embedded build identity.
pub fn bind(report: &mut Value) -> Result<(), String> {
    let declared = required("JEDEN_TEST_SOURCE_REVISION")?;
    let head = git(report, &["rev-parse", "HEAD"])?;
    if declared != head { return Err(format!("declared revision {declared} differs from checkout {head}")); }
    if !git(report, &["status", "--porcelain"])?.is_empty() {
        return Err("native runtime journey requires a clean committed candidate".into());
    }
    let count = git(report, &["rev-list", "--count", "HEAD"])?;
    let short = git(report, &["rev-parse", "--short", "HEAD"])?;
    let expected = format!("{}+dev.{count}.{short}", env!("CARGO_PKG_VERSION"));
    if env!("JEDEN_VERSION") != expected {
        return Err(format!("compiled journey identity {} differs from {expected}", env!("JEDEN_VERSION")));
    }
    let binary = env::current_exe().map_err(|cause| format!("locate native journey: {cause}"))?;
    let bytes = fs::read(&binary).map_err(|cause| format!("read journey binary: {cause}"))?;
    report["sourceRevision"] = json!(head);
    report["compiledVersion"] = json!(env!("JEDEN_VERSION"));
    report["binary"] = json!(binary);
    report["binarySha256"] = json!(format!("{:x}", Sha256::digest(bytes)));
    report["attestationSourceSha256"] = json!(format!("{:x}", Sha256::digest(include_bytes!("mod.rs"))));
    Ok(())
}
