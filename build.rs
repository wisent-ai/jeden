//! Build version identity is `<base>+dev.<commits>.<short-sha>[.dirty]`.
//! A non-empty `JEDEN_BUILD_VERSION` takes precedence over the generated identity.

use std::env;
use std::process::Command;

fn git(args: &[&str]) -> Option<String> {
    let output = Command::new("git").args(args).output().ok()?;
    output
        .status
        .success()
        .then(|| String::from_utf8_lossy(&output.stdout).trim().to_string())
}

fn main() {
    println!("cargo:rerun-if-env-changed=JEDEN_BUILD_VERSION");
    println!("cargo:rerun-if-changed=.git/HEAD");
    println!("cargo:rerun-if-changed=.git/index");

    if let Some(reference) = git(&["symbolic-ref", "-q", "HEAD"]) {
        println!("cargo:rerun-if-changed=.git/{reference}");
    }

    let base = env::var("CARGO_PKG_VERSION").expect("Cargo provides CARGO_PKG_VERSION");
    let version = env::var("JEDEN_BUILD_VERSION")
        .ok()
        .filter(|version| !version.is_empty())
        .unwrap_or_else(|| {
            let commits = git(&["rev-list", "--count", "HEAD"]).unwrap_or_else(|| "0".into());
            let sha = git(&["rev-parse", "--short", "HEAD"]).unwrap_or_else(|| "unknown".into());
            let dirty = git(&["status", "--porcelain"])
                .is_some_and(|status| !status.is_empty());
            format!(
                "{base}+dev.{commits}.{sha}{}",
                if dirty { ".dirty" } else { "" }
            )
        });

    println!("cargo:rustc-env=JEDEN_VERSION={version}");
}
