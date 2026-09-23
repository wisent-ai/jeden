//! Native staging: the binaries a release worker packages, built with the
//! declared private-source input, and the journeys that qualify them.

use super::cargo::{cargo, setting, OUTPUT_ENV};
use crate::repository_root;
use std::env;
use std::fs;

/// Build and copy the native binaries into `WISENT_OUTPUT_DIR/bin`, then run
/// each `qualifications` integration test's ignored journeys against that
/// staged candidate.
///
/// The journeys used to be the recipe's `tests` key. Stado 0.21.48, the
/// worker installed on ubuntu-server-rtx-pro-6000, predates that key and
/// refused jeden 0.1.23's recipe with `unknown recipe keys for this Stado:
/// tests`, so the build step runs them, as Stado's own recipe does.
pub(super) fn stage(binaries: &[String], qualifications: &[String]) -> Result<u8, String> {
    let Some(configured) = setting(OUTPUT_ENV) else {
        return Err(format!("{OUTPUT_ENV} is required for native staging"));
    };
    let output = std::path::absolute(&configured)
        .map_err(|error| format!("{OUTPUT_ENV}: {error}"))?
        .join("bin");
    let target = repository_root().join("target");
    let mut arguments = vec![
        "build".to_string(),
        "--release".into(),
        "--locked".into(),
        "--target-dir".into(),
        target.display().to_string(),
    ];
    for binary in binaries {
        arguments.push("--bin".into());
        arguments.push(binary.clone());
    }
    let tool = env::current_exe().map_err(|error| error.to_string())?;
    eprintln!("native stage: jeden-tools {}", tool.display());
    let status = cargo(&arguments)?;
    if status != 0 {
        return Ok(status);
    }
    fs::create_dir_all(&output).map_err(|error| format!("{}: {error}", output.display()))?;
    for binary in binaries {
        let name = format!("{binary}{}", env::consts::EXE_SUFFIX);
        let source = target.join("release").join(&name);
        let destination = output.join(&name);
        fs::copy(&source, &destination).map_err(|error| {
            format!("{} -> {}: {error}", source.display(), destination.display())
        })?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(&destination, fs::Permissions::from_mode(0o755))
                .map_err(|error| format!("{}: {error}", destination.display()))?;
        }
        eprintln!("native stage: {}", destination.display());
    }
    let qualification = target.join("qualification");
    for test in qualifications {
        eprintln!("native stage: qualifying the staged candidate with --test {test}");
        let status = cargo(&[
            "test".to_string(),
            "--locked".into(),
            "--release".into(),
            "--target-dir".into(),
            qualification.display().to_string(),
            "--test".into(),
            test.clone(),
            "--".into(),
            "--ignored".into(),
        ])?;
        if status != 0 {
            return Ok(status);
        }
    }
    Ok(0)
}
