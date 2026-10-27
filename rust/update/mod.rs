mod channel;
pub mod manifest;
pub mod transaction;

use semver::Version;
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use manifest::{ReleaseManifestV2, TrustRoot};
use transaction::{read_installed_state, InstallPaths};

pub use channel::{embedded_trust_roots, Channel};

pub struct UpdateRequest {
    pub manifest_location: String,
    pub channel: String,
    pub target_triple: String,
    pub target: PathBuf,
    pub roots: Vec<TrustRoot>,
    pub current_version: Version,
    pub now: Option<u64>,
    pub failpoint: Option<String>,
}

/// The target triple this binary was compiled for, which its updates are
/// released for; the build script stamps Cargo's own `TARGET`.
pub fn native_target_triple() -> String {
    env!("JEDEN_TARGET_TRIPLE").to_string()
}

mod fetch;

use fetch::{fetch, resolve};

fn verify_evidence(
    base: &str,
    reference: &str,
    artifact_sha256: &str,
    kind: &str,
) -> Result<(), String> {
    let (location, stated) = reference
        .rsplit_once("#sha256=")
        .ok_or_else(|| format!("{kind} reference must be digest-bound with #sha256=<hex>"))?;
    let expected = hex::decode(stated)
        .map_err(|error| format!("{kind} reference's SHA-256 {stated:?} is not hex: {error}"))?;
    let size = <Sha256 as Digest>::output_size();
    if expected.len() != size {
        return Err(format!(
            "{kind} reference's SHA-256 {stated:?} holds {} bytes, not a SHA-256 digest's {size}",
            expected.len()
        ));
    }
    let bytes = fetch(&resolve(base, location)?, None)?;
    let actual = Sha256::digest(&bytes);
    if actual.as_slice() != expected.as_slice() {
        return Err(format!(
            "{kind} digest mismatch: expected {}, got {}",
            hex::encode(&expected),
            hex::encode(actual)
        ));
    }
    let value: Value =
        serde_json::from_slice(&bytes).map_err(|error| format!("invalid {kind} JSON: {error}"))?;
    let bound = match kind {
        "provenance" => value
            .get("subject")
            .and_then(Value::as_array)
            .is_some_and(|subjects| {
                subjects.iter().any(|subject| {
                    subject.pointer("/digest/sha256").and_then(Value::as_str)
                        == Some(artifact_sha256)
                })
            }),
        "SBOM" => {
            let comment_bound = value
                .get("documentComment")
                .and_then(Value::as_str)
                .is_some_and(|comment| {
                    comment
                        .split_whitespace()
                        .any(|word| word.strip_prefix("sha256:") == Some(artifact_sha256))
                });
            let checksum_bound =
                value
                    .get("packages")
                    .and_then(Value::as_array)
                    .is_some_and(|packages| {
                        packages.iter().any(|package| {
                            package
                                .get("checksums")
                                .and_then(Value::as_array)
                                .is_some_and(|checksums| {
                                    checksums.iter().any(|checksum| {
                                        checksum.get("algorithm").and_then(Value::as_str)
                                            == Some("SHA256")
                                            && checksum.get("checksumValue").and_then(Value::as_str)
                                                == Some(artifact_sha256)
                                    })
                                })
                        })
                    });
            comment_bound || checksum_bound
        }
        _ => false,
    };
    if !bound {
        return Err(format!(
            "{kind} does not bind artifact digest {artifact_sha256}"
        ));
    }
    Ok(())
}

fn extract_release_executable(archive: &[u8], target_triple: &str) -> Result<Vec<u8>, String> {
    let expected_name = if target_triple.ends_with("-windows-msvc") {
        "jeden.exe"
    } else {
        "jeden"
    };
    let decoder = flate2::read::GzDecoder::new(archive);
    let mut tar = tar::Archive::new(decoder);
    let entries = tar
        .entries()
        .map_err(|error| format!("invalid release archive: {error}"))?;
    let mut executable = None;
    for entry in entries {
        let entry = entry.map_err(|error| format!("invalid release archive: {error}"))?;
        if !entry.header().entry_type().is_file() {
            return Err("release archive contains a non-file entry".into());
        }
        let path = entry
            .path()
            .map_err(|error| format!("invalid release archive path: {error}"))?;
        if path.to_str() != Some(expected_name) {
            return Err(format!(
                "release archive must contain only root-level {expected_name}"
            ));
        }
        if executable.is_some() {
            return Err("release archive contains multiple executable entries".into());
        }
        let declared_size = entry.size();
        let mut bytes = Vec::new();
        entry
            .take(declared_size)
            .read_to_end(&mut bytes)
            .map_err(|error| format!("read release executable: {error}"))?;
        if bytes.is_empty() {
            return Err("release executable is empty".into());
        }
        if bytes.len() as u64 != declared_size {
            return Err("release executable size does not match its archive header".into());
        }
        executable = Some(bytes);
    }
    executable.ok_or_else(|| format!("release archive is missing {expected_name}"))
}

/// Run the candidate's `capabilities` probe. A probe that fails says why: its
/// exit status and what it printed on stderr, which the rollback that follows
/// would otherwise leave unexplained.
pub fn run_health(binary: &Path, cwd: &Path) -> Result<(), String> {
    let output = stado_wait::output(
        Command::new(binary)
            .args(["capabilities", "--cwd"])
            .arg(cwd)
            .stdin(Stdio::null()),
    )
    .map_err(|error| format!("health probe failed to run {}: {error}", binary.display()))?;
    if output.status.success() {
        Ok(())
    } else {
        Err(format!(
            "health probe {} capabilities failed with {}: {}",
            binary.display(),
            output.status,
            String::from_utf8_lossy(&output.stderr).trim()
        ))
    }
}

pub fn execute(request: UpdateRequest) -> Result<ReleaseManifestV2, String> {
    if Channel::named(&request.channel).is_none() {
        return Err(format!(
            "update channel {:?} is none of {}",
            request.channel,
            Channel::names().join(", ")
        ));
    }
    let paths = InstallPaths::new(request.target)?;
    transaction::recover_exclusive(&paths)?;
    let current = match read_installed_state(&paths)? {
        Some(state) => Version::parse(&state.version)
            .map_err(|error| format!("invalid installed version state: {error}"))?,
        None => request.current_version,
    };
    let envelope = fetch(&request.manifest_location, None)?;
    let manifest = manifest::verify_envelope(
        &envelope,
        &request.roots,
        &request.channel,
        &request.target_triple,
        &current,
        request.now,
    )?;
    let artifact = fetch(
        &resolve(&request.manifest_location, &manifest.artifact_url)?,
        Some(manifest.size),
    )?;
    manifest::verify_artifact(&manifest, &artifact)?;
    verify_evidence(
        &request.manifest_location,
        &manifest.sbom_ref,
        &manifest.sha256,
        "SBOM",
    )?;
    verify_evidence(
        &request.manifest_location,
        &manifest.provenance_ref,
        &manifest.sha256,
        "provenance",
    )?;
    let executable = extract_release_executable(&artifact, &request.target_triple)?;
    transaction::install(
        &paths,
        &executable,
        &current.to_string(),
        &manifest.version,
        &manifest.sha256,
        request.failpoint.as_deref(),
        run_health,
    )?;
    Ok(manifest)
}
