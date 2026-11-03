//! Build version identity is `<base>+dev.<commits>.<short-sha>[.dirty]`.
//! A non-empty `JEDEN_BUILD_VERSION` takes precedence over the generated identity.
//! A tree exported without `.git` (Stado's release worker and install builds)
//! is identified by the commit Stado states in `WISENT_SOURCE_COMMIT`:
//! `<base>+commit.<short-sha>`, since the export holds no history to count.
//! Without either, a tree git cannot describe fails the build naming git's
//! answer and both variables.
//! The build also stamps the target triple it compiles for, which self-update
//! fetches the release of.

use std::env;
use std::process::Command;

/// What `git args` printed, or why it printed nothing usable: the spawn error,
/// or the exit status with git's own stderr.
fn git(args: &[&str]) -> Result<String, String> {
    let invocation = format!("git {}", args.join(" "));
    let output = stado_wait::output(Command::new("git").args(args))
        .map_err(|error| format!("{invocation} could not run: {error}"))?;
    if !output.status.success() {
        return Err(format!(
            "{invocation} failed ({}): {}",
            output.status,
            String::from_utf8_lossy(&output.stderr).trim()
        ));
    }
    Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
}

/// The identity git describes this tree with.
fn described(base: &str) -> Result<String, String> {
    let commits = git(&["rev-list", "--count", "HEAD"])?;
    let sha = git(&["rev-parse", "--short", "HEAD"])?;
    let status = git(&["status", "--porcelain"])?;
    let dirty = if status.is_empty() { "" } else { ".dirty" };
    Ok(format!("{base}+dev.{commits}.{sha}{dirty}"))
}

fn main() {
    println!("cargo:rerun-if-env-changed=JEDEN_BUILD_VERSION");
    println!("cargo:rerun-if-changed=.git/HEAD");
    println!("cargo:rerun-if-changed=.git/index");

    let base = env::var("CARGO_PKG_VERSION").expect("Cargo provides CARGO_PKG_VERSION");
    let target = env::var("TARGET").expect("Cargo provides TARGET to build scripts");
    println!("cargo:rustc-env=JEDEN_TARGET_TRIPLE={target}");

    let stated = match env::var("JEDEN_BUILD_VERSION") {
        Ok(version) if !version.is_empty() => Some(version),
        Ok(_) | Err(env::VarError::NotPresent) => None,
        Err(env::VarError::NotUnicode(value)) => {
            panic!("JEDEN_BUILD_VERSION is not Unicode: {value:?}")
        }
    };
    println!("cargo:rerun-if-env-changed=WISENT_SOURCE_COMMIT");
    let exported = match env::var("WISENT_SOURCE_COMMIT") {
        Ok(commit) if commit.len() >= 7 && commit.bytes().all(|byte| byte.is_ascii_hexdigit()) => {
            Some(format!("{base}+commit.{}", &commit[..7]))
        }
        Ok(commit) if commit.is_empty() => None,
        Ok(commit) => panic!(
            "WISENT_SOURCE_COMMIT {commit:?} is not a commit id (at least seven hex digits)"
        ),
        Err(env::VarError::NotPresent) => None,
        Err(env::VarError::NotUnicode(value)) => {
            panic!("WISENT_SOURCE_COMMIT is not Unicode: {value:?}")
        }
    };
    let version = match (stated, exported) {
        (Some(version), _) => version,
        (None, Some(version)) => version,
        (None, None) => {
            const WAY_OUT: &str =
                "set JEDEN_BUILD_VERSION, or WISENT_SOURCE_COMMIT for an exported tree, to build without git";
            // The branch HEAD names is watched too, so a commit on it rebuilds;
            // a detached HEAD names no branch, and `.git/HEAD` itself changes.
            match git(&["rev-parse", "--symbolic-full-name", "HEAD"]) {
                Ok(reference) if reference != "HEAD" => {
                    println!("cargo:rerun-if-changed=.git/{reference}");
                }
                Ok(_detached) => {}
                Err(cause) => panic!("{cause}; {WAY_OUT}"),
            }
            match described(&base) {
                Ok(version) => version,
                Err(cause) => panic!("{cause}; {WAY_OUT}"),
            }
        }
    };

    println!("cargo:rustc-env=JEDEN_VERSION={version}");
}
