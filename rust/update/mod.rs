pub mod manifest;
pub mod transaction;

use base64::Engine;
use ed25519_dalek::VerifyingKey;
use semver::Version;
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use manifest::{ReleaseManifestV2, TrustRoot};
use transaction::{read_installed_state, InstallPaths};

const MAX_DOWNLOAD_BYTES: usize = 256 * 1024 * 1024;
const PROBE_TIMEOUT: Duration = Duration::from_secs(10);
const CANARY_RELEASE_KEY_ID: &str = "jeden-canary-2026-07-13";
const CANARY_RELEASE_PUBLIC_KEY: &str = "8hCBoR81Kax1U4oPKyg0C9IvYifV+o+6qc4L6JYbCFk=";
const STABLE_RELEASE_KEY_ID: &str = "jeden-stable-2026-07-13";
const STABLE_RELEASE_PUBLIC_KEY: &str = "78wFp2XYBVMWv/MfkvTlQ3TqWjyHMgJWKA9KK4e9wsA=";

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

pub fn embedded_trust_roots() -> Result<Vec<TrustRoot>, String> {
    let canary = (CANARY_RELEASE_KEY_ID, CANARY_RELEASE_PUBLIC_KEY);
    let stable = (STABLE_RELEASE_KEY_ID, STABLE_RELEASE_PUBLIC_KEY);
    [("canary", canary), ("stable", stable)]
        .into_iter()
        .map(|(channel, (key_id, encoded))| {
            let bytes = base64::engine::general_purpose::STANDARD
                .decode(encoded)
                .map_err(|_| "embedded trust root is invalid base64")?;
            let bytes: [u8; 32] = bytes
                .try_into()
                .map_err(|_| "embedded trust root is not 32 bytes")?;
            let key = VerifyingKey::from_bytes(&bytes)
                .map_err(|_| "embedded trust root is not a valid Ed25519 key")?;
            Ok(TrustRoot {
                channel: channel.into(),
                key_id: key_id.into(),
                key,
            })
        })
        .collect()
}

pub fn native_target_triple() -> Result<String, String> {
    let triple = match (std::env::consts::ARCH, std::env::consts::OS) {
        ("aarch64", "macos") => "aarch64-apple-darwin",
        ("x86_64", "macos") => "x86_64-apple-darwin",
        ("aarch64", "linux") => "aarch64-unknown-linux-gnu",
        ("x86_64", "linux") => "x86_64-unknown-linux-gnu",
        ("x86_64", "windows") => "x86_64-pc-windows-msvc",
        ("aarch64", "windows") => "aarch64-pc-windows-msvc",
        (arch, os) => return Err(format!("self-update target is unsupported for {arch}-{os}")),
    };
    Ok(triple.into())
}

fn github_auth_token(location: &str) -> Option<String> {
    let url = reqwest::Url::parse(location).ok()?;
    if url.scheme() != "https" {
        return None;
    }
    let path = url.path().to_ascii_lowercase();
    let authorized_path = match url.host_str() {
        Some("github.com") => path == "/wisent-ai/jeden" || path.starts_with("/wisent-ai/jeden/"),
        Some("api.github.com") => {
            path == "/repos/wisent-ai/jeden" || path.starts_with("/repos/wisent-ai/jeden/")
        }
        _ => false,
    };
    if !authorized_path {
        return None;
    }
    ["JEDEN_UPDATE_GITHUB_TOKEN", "GH_TOKEN"]
        .into_iter()
        .find_map(|name| {
            std::env::var(name)
                .ok()
                .map(|value| value.trim().to_owned())
                .filter(|value| !value.is_empty())
        })
}

fn safe_github_segment(value: &str) -> bool {
    !value.is_empty()
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-'))
}

fn github_release_asset_coordinates(location: &str) -> Option<(String, String)> {
    let url = reqwest::Url::parse(location).ok()?;
    if url.scheme() != "https" || url.host_str() != Some("github.com") {
        return None;
    }
    let segments = url.path_segments()?.collect::<Vec<_>>();
    if segments.len() != 6
        || !segments[0].eq_ignore_ascii_case("wisent-ai")
        || !segments[1].eq_ignore_ascii_case("jeden")
        || segments[2] != "releases"
        || segments[3] != "download"
        || !safe_github_segment(segments[4])
        || !safe_github_segment(segments[5])
    {
        return None;
    }
    Some((segments[4].to_owned(), segments[5].to_owned()))
}

fn github_asset_api_url(metadata: &Value, asset_name: &str) -> Option<String> {
    let raw = metadata
        .get("assets")?
        .as_array()?
        .iter()
        .find(|asset| asset.get("name").and_then(Value::as_str) == Some(asset_name))?
        .get("url")?
        .as_str()?;
    let url = reqwest::Url::parse(raw).ok()?;
    let segments = url.path_segments()?.collect::<Vec<_>>();
    if url.scheme() != "https"
        || url.host_str() != Some("api.github.com")
        || segments.len() != 6
        || segments[0] != "repos"
        || !segments[1].eq_ignore_ascii_case("wisent-ai")
        || !segments[2].eq_ignore_ascii_case("jeden")
        || segments[3] != "releases"
        || segments[4] != "assets"
        || segments[5].is_empty()
        || !segments[5].bytes().all(|byte| byte.is_ascii_digit())
    {
        return None;
    }
    Some(url.to_string())
}

fn github_release_asset_request(
    client: &reqwest::blocking::Client,
    location: &str,
    token: &str,
) -> Result<Option<reqwest::blocking::RequestBuilder>, String> {
    let Some((tag, asset_name)) = github_release_asset_coordinates(location) else {
        return Ok(None);
    };
    let endpoint = format!("https://api.github.com/repos/wisent-ai/jeden/releases/tags/{tag}");
    let response = client
        .get(endpoint)
        .bearer_auth(token)
        .header(reqwest::header::ACCEPT, "application/vnd.github+json")
        .header(reqwest::header::USER_AGENT, "jeden-updater")
        .send()
        .map_err(|error| format!("resolve private GitHub release asset: {error}"))?;
    if !response.status().is_success() {
        return Err(format!(
            "resolve private GitHub release asset returned {}",
            response.status()
        ));
    }
    if response
        .content_length()
        .is_some_and(|size| size > 1024 * 1024)
    {
        return Err("private GitHub release metadata exceeds size limit".into());
    }
    let bytes = response.bytes().map_err(|error| error.to_string())?;
    if bytes.len() > 1024 * 1024 {
        return Err("private GitHub release metadata exceeds size limit".into());
    }
    let metadata: Value = serde_json::from_slice(&bytes)
        .map_err(|error| format!("invalid private GitHub release metadata: {error}"))?;
    let asset_url = github_asset_api_url(&metadata, &asset_name)
        .ok_or_else(|| format!("private GitHub release asset is unavailable: {asset_name}"))?;
    Ok(Some(
        client
            .get(asset_url)
            .bearer_auth(token)
            .header(reqwest::header::ACCEPT, "application/octet-stream")
            .header(reqwest::header::USER_AGENT, "jeden-updater"),
    ))
}

fn fetch(location: &str, limit: usize) -> Result<Vec<u8>, String> {
    if location.starts_with("https://") {
        let client = reqwest::blocking::Client::builder()
            .timeout(Duration::from_secs(30))
            .build()
            .map_err(|error| error.to_string())?;
        let mut request = client.get(location);
        if let Some(token) = github_auth_token(location) {
            request = github_release_asset_request(&client, location, &token)?
                .unwrap_or_else(|| client.get(location).bearer_auth(token));
        }
        let response = request
            .send()
            .map_err(|error| format!("download {location}: {error}"))?;
        if !response.status().is_success() {
            return Err(format!(
                "download {location} returned {}",
                response.status()
            ));
        }
        if response
            .content_length()
            .is_some_and(|size| size > limit as u64)
        {
            return Err(format!("download {location} exceeds size limit"));
        }
        let bytes = response.bytes().map_err(|error| error.to_string())?;
        if bytes.len() > limit {
            return Err(format!("download {location} exceeds size limit"));
        }
        return Ok(bytes.to_vec());
    }
    let path = location.strip_prefix("file://").unwrap_or(location);
    let metadata = std::fs::metadata(path).map_err(|error| format!("read {path}: {error}"))?;
    if metadata.len() > limit as u64 {
        return Err(format!("read {path}: file exceeds size limit"));
    }
    std::fs::read(path).map_err(|error| format!("read {path}: {error}"))
}

fn resolve(base: &str, reference: &str) -> String {
    if reference.contains("://") || Path::new(reference).is_absolute() {
        return reference.into();
    }
    if base.starts_with("https://") {
        return reqwest::Url::parse(base)
            .ok()
            .and_then(|url| url.join(reference).ok())
            .map(|url| url.to_string())
            .unwrap_or_else(|| reference.into());
    }
    Path::new(base.strip_prefix("file://").unwrap_or(base))
        .parent()
        .unwrap_or_else(|| Path::new("."))
        .join(reference)
        .display()
        .to_string()
}

fn verify_evidence(
    base: &str,
    reference: &str,
    artifact_sha256: &str,
    kind: &str,
) -> Result<(), String> {
    let (location, expected) = reference
        .rsplit_once("#sha256=")
        .ok_or_else(|| format!("{kind} reference must be digest-bound with #sha256=<hex>"))?;
    if expected.len() != 64 || !expected.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return Err(format!("{kind} reference has invalid SHA-256"));
    }
    let bytes = fetch(&resolve(base, location), 16 * 1024 * 1024)?;
    let actual = hex::encode(Sha256::digest(&bytes));
    if actual != expected.to_ascii_lowercase() {
        return Err(format!(
            "{kind} digest mismatch: expected {expected}, got {actual}"
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
        if path.components().count() != 1 || path.to_str() != Some(expected_name) {
            return Err(format!(
                "release archive must contain only root-level {expected_name}"
            ));
        }
        if executable.is_some() {
            return Err("release archive contains multiple executable entries".into());
        }
        let declared_size = entry.size();
        if declared_size == 0 || declared_size > MAX_DOWNLOAD_BYTES as u64 {
            return Err("release executable is empty or exceeds size limit".into());
        }
        let mut bytes = Vec::with_capacity(declared_size as usize);
        entry
            .take(MAX_DOWNLOAD_BYTES as u64 + 1)
            .read_to_end(&mut bytes)
            .map_err(|error| format!("read release executable: {error}"))?;
        if bytes.len() as u64 != declared_size || bytes.len() > MAX_DOWNLOAD_BYTES {
            return Err("release executable size does not match its archive header".into());
        }
        executable = Some(bytes);
    }
    executable.ok_or_else(|| format!("release archive is missing {expected_name}"))
}

pub fn run_health(binary: &Path, cwd: &Path) -> Result<(), String> {
    let started = Instant::now();
    let mut child = Command::new(binary)
        .args(["capabilities", "--cwd"])
        .arg(cwd)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|error| format!("health probe failed to start {}: {error}", binary.display()))?;
    loop {
        match child.try_wait() {
            Ok(Some(status)) if status.success() => return Ok(()),
            Ok(Some(status)) => return Err(format!("health probe failed with {status}")),
            Ok(None) if started.elapsed() < PROBE_TIMEOUT => {
                std::thread::sleep(Duration::from_millis(20))
            }
            Ok(None) => {
                let _ = child.kill();
                let _ = child.wait();
                return Err("health probe timed out".into());
            }
            Err(error) => {
                let _ = child.kill();
                return Err(format!("health probe failed: {error}"));
            }
        }
    }
}

pub fn execute(request: UpdateRequest) -> Result<ReleaseManifestV2, String> {
    if !matches!(request.channel.as_str(), "canary" | "stable") {
        return Err("update channel must be canary or stable".into());
    }
    let paths = InstallPaths::new(request.target)?;
    transaction::recover_exclusive(&paths)?;
    let current = match read_installed_state(&paths)? {
        Some(state) => Version::parse(&state.version)
            .map_err(|error| format!("invalid installed version state: {error}"))?,
        None => request.current_version,
    };
    let envelope = fetch(&request.manifest_location, 1024 * 1024)?;
    let manifest = manifest::verify_envelope(
        &envelope,
        &request.roots,
        &request.channel,
        &request.target_triple,
        &current,
        request.now,
    )?;
    let artifact = fetch(
        &resolve(&request.manifest_location, &manifest.artifact_url),
        MAX_DOWNLOAD_BYTES,
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
