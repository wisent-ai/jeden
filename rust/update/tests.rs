use super::manifest::{self, ReleaseManifestV2, TrustRoot, PAYLOAD_TYPE};
use base64::Engine;
use ed25519_dalek::{Signer, SigningKey};
use flate2::{Compression, GzBuilder};
use semver::Version;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use std::ffi::OsString;
use std::io::Write;
use std::sync::Mutex;

struct TestArchiveEntry<'a> {
    path: &'a str,
    entry_type: tar::EntryType,
    contents: &'a [u8],
    link_name: Option<&'a str>,
}

fn archive_header(path: &str, entry_type: tar::EntryType, size: u64) -> tar::Header {
    assert!(
        path.len() < 100,
        "test archive path must fit in a tar header"
    );
    let mut header = tar::Header::new_gnu();
    header.set_mode(0o755);
    header.set_uid(0);
    header.set_gid(0);
    header.set_mtime(0);
    header.set_size(size);
    header.set_entry_type(entry_type);
    let name = &mut header.as_mut_bytes()[..100];
    name.fill(0);
    name[..path.len()].copy_from_slice(path.as_bytes());
    header
}

fn tar_gz(entries: &[TestArchiveEntry<'_>]) -> Vec<u8> {
    let encoder = GzBuilder::new()
        .mtime(0)
        .write(Vec::new(), Compression::default());
    let mut builder = tar::Builder::new(encoder);
    for entry in entries {
        let mut header = archive_header(entry.path, entry.entry_type, entry.contents.len() as u64);
        if let Some(link_name) = entry.link_name {
            header.set_link_name(link_name).unwrap();
        }
        header.set_cksum();
        builder.append(&header, entry.contents).unwrap();
    }
    builder.finish().unwrap();
    builder.into_inner().unwrap().finish().unwrap()
}

fn gzip(bytes: &[u8]) -> Vec<u8> {
    let mut encoder = GzBuilder::new()
        .mtime(0)
        .write(Vec::new(), Compression::default());
    encoder.write_all(bytes).unwrap();
    encoder.finish().unwrap()
}

fn regular<'a>(path: &'a str, contents: &'a [u8]) -> TestArchiveEntry<'a> {
    TestArchiveEntry {
        path,
        entry_type: tar::EntryType::Regular,
        contents,
        link_name: None,
    }
}

fn assert_archive_rejected(name: &str, archive: &[u8], target: &str) {
    assert!(
        super::extract_release_executable(archive, target).is_err(),
        "{name}: unsafe archive was accepted"
    );
}

const NOW: u64 = 1_800_000_000;
const TARGET: &str = "aarch64-apple-darwin";
const CANARY_RELEASE_KEY_ID: &str = "jeden-canary-2026-07-13";
const CANARY_RELEASE_PUBLIC_KEY: &str = "8hCBoR81Kax1U4oPKyg0C9IvYifV+o+6qc4L6JYbCFk=";
const STABLE_RELEASE_KEY_ID: &str = "jeden-stable-2026-07-13";
const STABLE_RELEASE_PUBLIC_KEY: &str = "78wFp2XYBVMWv/MfkvTlQ3TqWjyHMgJWKA9KK4e9wsA=";

const UPDATE_TOKEN_ENV_NAMES: [&str; 2] = ["JEDEN_UPDATE_GITHUB_TOKEN", "GH_TOKEN"];
static UPDATE_TOKEN_ENVIRONMENT: Mutex<()> = Mutex::new(());

struct UpdateTokenEnvironment(Vec<(&'static str, Option<OsString>)>);

impl UpdateTokenEnvironment {
    fn isolated() -> Self {
        let previous = UPDATE_TOKEN_ENV_NAMES
            .into_iter()
            .map(|name| (name, std::env::var_os(name)))
            .collect();
        for name in UPDATE_TOKEN_ENV_NAMES {
            std::env::remove_var(name);
        }
        Self(previous)
    }

    fn set(name: &str, value: Option<&str>) {
        match value {
            Some(value) => std::env::set_var(name, value),
            None => std::env::remove_var(name),
        }
    }
}

impl Drop for UpdateTokenEnvironment {
    fn drop(&mut self) {
        for (name, value) in self.0.drain(..) {
            match value {
                Some(value) => std::env::set_var(name, value),
                None => std::env::remove_var(name),
            }
        }
    }
}

fn signing_key(seed: u8) -> SigningKey {
    SigningKey::from_bytes(&[seed; 32])
}

fn trust_root(key: &SigningKey, channel: &str, key_id: &str) -> TrustRoot {
    TrustRoot {
        channel: channel.into(),
        key_id: key_id.into(),
        key: key.verifying_key(),
    }
}

fn manifest() -> ReleaseManifestV2 {
    let artifact = b"release artifact";
    ReleaseManifestV2 {
        schema_version: 2,
        version: "2.0.0".into(),
        channel: "stable".into(),
        target_triple: TARGET.into(),
        artifact_url: "jeden".into(),
        sha256: hex::encode(Sha256::digest(artifact)),
        size: artifact.len() as u64,
        published_at: "2027-01-01T00:00:00Z".into(),
        expires_at: "2027-02-01T00:00:00Z".into(),
        minimum_version: "1.0.0".into(),
        key_id: "ephemeral-test-root".into(),
        provenance_ref: "provenance.json#sha256=aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa".into(),
        sbom_ref: "sbom.json#sha256=bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb".into(),
    }
}

fn canonical_payload(value: &ReleaseManifestV2) -> Vec<u8> {
    let value = serde_json::to_value(value).unwrap();
    let object = value.as_object().unwrap();
    let ordered: BTreeMap<&str, &Value> = object
        .iter()
        .map(|(key, value)| (key.as_str(), value))
        .collect();
    serde_json::to_vec(&ordered).unwrap()
}

fn pae(payload: &[u8]) -> Vec<u8> {
    let mut bytes = format!(
        "DSSEv1 {} {} {} ",
        PAYLOAD_TYPE.len(),
        PAYLOAD_TYPE,
        payload.len()
    )
    .into_bytes();
    bytes.extend_from_slice(payload);
    bytes
}

fn signed_envelope(value: &ReleaseManifestV2, key: &SigningKey) -> Vec<u8> {
    let payload = canonical_payload(value);
    let signature = key.sign(&pae(&payload));
    serde_json::to_vec(&json!({
        "payloadType": PAYLOAD_TYPE,
        "payload": base64::engine::general_purpose::STANDARD.encode(payload),
        "signatures": [{
            "keyid": value.key_id,
            "sig": base64::engine::general_purpose::STANDARD.encode(signature.to_bytes()),
        }],
    }))
    .unwrap()
}

fn verify(value: &ReleaseManifestV2, key: &SigningKey) -> Result<ReleaseManifestV2, String> {
    manifest::verify_envelope(
        &signed_envelope(value, key),
        &[trust_root(key, "stable", &value.key_id)],
        "stable",
        TARGET,
        &Version::parse("1.0.0").unwrap(),
        Some(NOW),
    )
}

#[test]
fn accepts_canonical_dsse_ed25519_manifest_from_deterministic_test_root() {
    let key = signing_key(1);
    let expected = manifest();

    let verified = verify(&expected, &key).unwrap();

    assert_eq!(verified, expected);
}

#[test]
fn embedded_trust_roots_exactly_match_release_authority() {
    let roots = super::embedded_trust_roots().unwrap();
    let actual: Vec<_> = roots
        .iter()
        .map(|root| {
            (
                root.channel.as_str(),
                root.key_id.as_str(),
                base64::engine::general_purpose::STANDARD.encode(root.key.to_bytes()),
            )
        })
        .collect();

    assert_eq!(
        actual,
        vec![
            (
                "canary",
                CANARY_RELEASE_KEY_ID,
                CANARY_RELEASE_PUBLIC_KEY.to_owned(),
            ),
            (
                "stable",
                STABLE_RELEASE_KEY_ID,
                STABLE_RELEASE_PUBLIC_KEY.to_owned(),
            ),
        ]
    );
}

#[test]
fn dsse_verification_requires_matching_root_channel_and_key_id() {
    let key = signing_key(2);
    let mut value = manifest();
    value.channel = "canary".into();
    value.key_id = "deterministic-canary-test-root".into();
    let envelope = signed_envelope(&value, &key);

    let verified = manifest::verify_envelope(
        &envelope,
        &[trust_root(&key, "canary", &value.key_id)],
        "canary",
        TARGET,
        &Version::parse("1.0.0").unwrap(),
        Some(NOW),
    )
    .unwrap();
    assert_eq!(verified, value);

    for (name, root) in [
        (
            "channel",
            trust_root(&key, "stable", "deterministic-canary-test-root"),
        ),
        (
            "key ID",
            trust_root(&key, "canary", "different-deterministic-test-root"),
        ),
    ] {
        let error = manifest::verify_envelope(
            &envelope,
            &[root],
            "canary",
            TARGET,
            &Version::parse("1.0.0").unwrap(),
            Some(NOW),
        )
        .unwrap_err();
        assert!(error.contains("untrusted release key"), "{name}: {error}");
    }
}

#[test]
fn rejects_payload_tampering_after_signature() {
    let key = signing_key(3);
    let value = manifest();
    let mut envelope: Value = serde_json::from_slice(&signed_envelope(&value, &key)).unwrap();
    let payload = base64::engine::general_purpose::STANDARD
        .decode(envelope["payload"].as_str().unwrap())
        .unwrap();
    let mut tampered: BTreeMap<String, Value> = serde_json::from_slice(&payload).unwrap();
    tampered.insert(
        "artifactUrl".into(),
        Value::String("attacker-artifact".into()),
    );
    envelope["payload"] = Value::String(
        base64::engine::general_purpose::STANDARD.encode(serde_json::to_vec(&tampered).unwrap()),
    );

    let error = manifest::verify_envelope(
        &serde_json::to_vec(&envelope).unwrap(),
        &[trust_root(&key, "stable", &value.key_id)],
        "stable",
        TARGET,
        &Version::parse("1.0.0").unwrap(),
        Some(NOW),
    )
    .unwrap_err();

    assert!(error.contains("signature verification failed"), "{error}");
}

#[test]
fn rejects_signature_from_wrong_key() {
    let signer = signing_key(4);
    let trusted = signing_key(5);
    let value = manifest();

    let error = manifest::verify_envelope(
        &signed_envelope(&value, &signer),
        &[trust_root(&trusted, "stable", &value.key_id)],
        "stable",
        TARGET,
        &Version::parse("1.0.0").unwrap(),
        Some(NOW),
    )
    .unwrap_err();

    assert!(error.contains("signature verification failed"), "{error}");
}

#[test]
fn rejects_release_for_another_channel_or_target() {
    let key = signing_key(6);
    let value = manifest();
    for (name, channel, target, expected) in [
        ("channel", "canary", TARGET, "release channel mismatch"),
        (
            "target",
            "stable",
            "x86_64-unknown-linux-gnu",
            "release target mismatch",
        ),
    ] {
        let error = manifest::verify_envelope(
            &signed_envelope(&value, &key),
            &[trust_root(&key, "stable", &value.key_id)],
            channel,
            target,
            &Version::parse("1.0.0").unwrap(),
            Some(NOW),
        )
        .unwrap_err();
        assert!(error.contains(expected), "{name}: {error}");
    }
}

#[test]
fn rejects_expired_and_unreasonably_future_manifests() {
    let key = signing_key(7);
    for (name, published_at, expires_at, expected) in [
        (
            "expired",
            "2026-12-01T00:00:00Z",
            "2027-01-01T00:00:00Z",
            "expired or has an invalid validity window",
        ),
        (
            "future",
            "2027-02-01T00:00:00Z",
            "2027-03-01T00:00:00Z",
            "publication time is in the future",
        ),
    ] {
        let mut value = manifest();
        value.published_at = published_at.into();
        value.expires_at = expires_at.into();
        let error = verify(&value, &key).unwrap_err();
        assert!(error.contains(expected), "{name}: {error}");
    }
}

#[test]
fn rejects_downgrade_and_replay_versions() {
    let key = signing_key(8);
    for (name, candidate) in [("replay", "1.5.0"), ("downgrade", "1.4.9")] {
        let mut value = manifest();
        value.version = candidate.into();
        let error = manifest::verify_envelope(
            &signed_envelope(&value, &key),
            &[trust_root(&key, "stable", &value.key_id)],
            "stable",
            TARGET,
            &Version::parse("1.5.0").unwrap(),
            Some(NOW),
        )
        .unwrap_err();
        assert!(
            error.contains("downgrade/replay refused"),
            "{name}: {error}"
        );
    }
}

#[test]
fn artifact_verification_rejects_size_and_checksum_mismatches() {
    let artifact = b"release artifact";
    let valid = manifest();
    manifest::verify_artifact(&valid, artifact).unwrap();

    let mut wrong_size = valid.clone();
    wrong_size.size += 1;
    let error = manifest::verify_artifact(&wrong_size, artifact).unwrap_err();
    assert!(error.contains("size mismatch"), "{error}");

    let same_size_tamper = b"release artifacu";
    assert_eq!(same_size_tamper.len(), artifact.len());
    let error = manifest::verify_artifact(&valid, same_size_tamper).unwrap_err();
    assert!(error.contains("checksum mismatch"), "{error}");
}

#[test]
fn extracts_one_root_executable_for_unix_and_windows_targets() {
    let unix_payload = b"#!/bin/sh\nexit 0\n";
    let unix_archive = tar_gz(&[regular("jeden", unix_payload)]);
    assert_eq!(
        super::extract_release_executable(&unix_archive, "x86_64-unknown-linux-gnu").unwrap(),
        unix_payload
    );

    let windows_payload = b"deterministic PE fixture";
    let windows_archive = tar_gz(&[regular("jeden.exe", windows_payload)]);
    assert_eq!(
        super::extract_release_executable(&windows_archive, "x86_64-pc-windows-msvc").unwrap(),
        windows_payload
    );
}

#[test]
fn rejects_empty_multiple_extra_and_wrong_name_archives() {
    let cases = [
        ("empty", tar_gz(&[])),
        (
            "duplicate executable",
            tar_gz(&[regular("jeden", b"first"), regular("jeden", b"second")]),
        ),
        (
            "extra regular member",
            tar_gz(&[
                regular("jeden", b"executable"),
                regular("notes.txt", b"extra"),
            ]),
        ),
        (
            "wrong executable name",
            tar_gz(&[regular("other", b"executable")]),
        ),
        (
            "Windows name on Unix",
            tar_gz(&[regular("jeden.exe", b"executable")]),
        ),
    ];

    for (name, archive) in cases {
        assert_archive_rejected(name, &archive, "aarch64-apple-darwin");
    }
    assert_archive_rejected(
        "Unix name on Windows",
        &tar_gz(&[regular("jeden", b"executable")]),
        "aarch64-pc-windows-msvc",
    );
}

#[test]
fn rejects_non_regular_archive_members() {
    for (name, entry_type, link_name) in [
        ("directory", tar::EntryType::Directory, None),
        ("symlink", tar::EntryType::Symlink, Some("target")),
        ("hardlink", tar::EntryType::Link, Some("target")),
    ] {
        let archive = tar_gz(&[TestArchiveEntry {
            path: "jeden",
            entry_type,
            contents: b"",
            link_name,
        }]);
        assert_archive_rejected(name, &archive, "x86_64-unknown-linux-gnu");
    }
}

#[test]
fn rejects_nested_and_traversal_member_paths() {
    for path in ["bin/jeden", "../jeden", "/jeden"] {
        let archive = tar_gz(&[regular(path, b"executable")]);
        assert_archive_rejected(path, &archive, "x86_64-unknown-linux-gnu");
    }
}

#[test]
fn rejects_malformed_gzip_and_tar_streams() {
    assert_archive_rejected(
        "malformed gzip",
        b"not a gzip stream",
        "x86_64-unknown-linux-gnu",
    );
    assert_archive_rejected(
        "malformed tar",
        &gzip(b"not a complete tar header"),
        "x86_64-unknown-linux-gnu",
    );
}

#[test]
fn rejects_declared_payload_over_cap_without_allocating_the_payload() {
    let mut header = archive_header(
        "jeden",
        tar::EntryType::Regular,
        super::MAX_DOWNLOAD_BYTES as u64 + 1,
    );
    header.set_cksum();
    let archive = gzip(header.as_bytes());

    let error =
        super::extract_release_executable(&archive, "x86_64-unknown-linux-gnu").unwrap_err();
    assert!(error.contains("large") || error.contains("size"), "{error}");
}

#[test]
fn artifact_digest_verification_stays_bound_to_compressed_archive() {
    let payload = b"#!/bin/sh\nexit 0\n";
    let archive = tar_gz(&[regular("jeden", payload)]);
    let mut release = manifest();
    release.size = archive.len() as u64;
    release.sha256 = hex::encode(Sha256::digest(&archive));

    manifest::verify_artifact(&release, &archive).unwrap();
    assert_eq!(
        super::extract_release_executable(&archive, "aarch64-apple-darwin").unwrap(),
        payload
    );
    assert!(manifest::verify_artifact(&release, payload).is_err());
}
#[test]
fn github_auth_token_prefers_explicit_token_and_ignores_empty_values() {
    let _guard = UPDATE_TOKEN_ENVIRONMENT
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let _environment = UpdateTokenEnvironment::isolated();
    let release = "https://github.com/wisent-ai/jeden/releases/download/v1/manifest.json";

    UpdateTokenEnvironment::set("JEDEN_UPDATE_GITHUB_TOKEN", Some("explicit-secret"));
    UpdateTokenEnvironment::set("GH_TOKEN", Some("gh-secret"));
    assert_eq!(
        super::github_auth_token(release).as_deref(),
        Some("explicit-secret")
    );

    UpdateTokenEnvironment::set("JEDEN_UPDATE_GITHUB_TOKEN", Some(""));
    assert_eq!(
        super::github_auth_token(release).as_deref(),
        Some("gh-secret")
    );

    UpdateTokenEnvironment::set("JEDEN_UPDATE_GITHUB_TOKEN", Some("   "));
    assert_eq!(
        super::github_auth_token(release).as_deref(),
        Some("gh-secret")
    );

    UpdateTokenEnvironment::set("GH_TOKEN", Some(""));
    assert_eq!(super::github_auth_token(release), None);

    UpdateTokenEnvironment::set("JEDEN_UPDATE_GITHUB_TOKEN", None);
    UpdateTokenEnvironment::set("GH_TOKEN", None);
    assert_eq!(super::github_auth_token(release), None);
}

#[test]
fn github_auth_token_is_limited_to_exact_github_https_hosts() {
    let _guard = UPDATE_TOKEN_ENVIRONMENT
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let _environment = UpdateTokenEnvironment::isolated();
    UpdateTokenEnvironment::set("JEDEN_UPDATE_GITHUB_TOKEN", Some("scoped-secret"));

    for location in [
        "https://github.com/wisent-ai/jeden/releases/download/v1/artifact",
        "https://api.github.com/repos/wisent-ai/jeden/releases/latest",
    ] {
        assert_eq!(
            super::github_auth_token(location).as_deref(),
            Some("scoped-secret"),
            "expected authentication for {location}"
        );
    }

    for location in [
        "http://github.com/wisent-ai/jeden/releases/download/v1/artifact",
        "http://api.github.com/repos/wisent-ai/jeden/releases/latest",
        "https://evil.github.com/wisent-ai/jeden/releases/latest",
        "https://github.com.evil.example/wisent-ai/jeden/releases/latest",
        "https://api.github.com.evil.example/repos/wisent-ai/jeden/releases/latest",
        "https://github.com@evil.example/wisent-ai/jeden/releases/latest",
        "https://github.com/wisent-ai/jeden-malicious/releases/latest",
        "https://github.com/another-owner/another-repo/releases/latest",
        "https://api.github.com/repos/wisent-ai/jeden-malicious/releases/latest",
        "https://api.github.com/repos/another-owner/another-repo/releases/latest",
        "https://raw.githubusercontent.com/wisent-ai/jeden/main/manifest.json",
        "file:///tmp/manifest.json",
    ] {
        assert_eq!(
            super::github_auth_token(location),
            None,
            "must not authenticate {location}"
        );
    }
}

#[test]
fn github_release_asset_coordinates_require_exact_safe_download_shape() {
    assert_eq!(
        super::github_release_asset_coordinates(
            "https://github.com/wisent-ai/jeden/releases/download/v1.2.3/jeden-aarch64.tar.gz",
        ),
        Some(("v1.2.3".into(), "jeden-aarch64.tar.gz".into()))
    );

    for location in [
        "http://github.com/wisent-ai/jeden/releases/download/v1.2.3/jeden.tar.gz",
        "https://api.github.com/wisent-ai/jeden/releases/download/v1.2.3/jeden.tar.gz",
        "https://evil.github.com/wisent-ai/jeden/releases/download/v1.2.3/jeden.tar.gz",
        "https://github.com.evil.example/wisent-ai/jeden/releases/download/v1.2.3/jeden.tar.gz",
        "https://github.com/another-owner/jeden/releases/download/v1.2.3/jeden.tar.gz",
        "https://github.com/wisent-ai/another-repo/releases/download/v1.2.3/jeden.tar.gz",
        "https://github.com/wisent-ai/jeden/release/download/v1.2.3/jeden.tar.gz",
        "https://github.com/wisent-ai/jeden/releases/v1.2.3/jeden.tar.gz",
        "https://github.com/wisent-ai/jeden/releases/download/v1.2.3",
        "https://github.com/wisent-ai/jeden/releases/download/v1.2.3/jeden.tar.gz/extra",
        "https://github.com/wisent-ai/jeden/releases/download/v1%2Fescape/jeden.tar.gz",
        "https://github.com/wisent-ai/jeden/releases/download/v1.2.3/jeden%2Fescape.tar.gz",
        "https://github.com/wisent-ai/jeden/releases/download/v1@escape/jeden.tar.gz",
        "https://github.com/wisent-ai/jeden/releases/download/v1.2.3/jeden%20escape.tar.gz",
    ] {
        assert_eq!(
            super::github_release_asset_coordinates(location),
            None,
            "must not resolve unsafe release URL {location}"
        );
    }
}

#[test]
fn github_asset_api_url_accepts_only_jeden_numeric_asset_endpoints() {
    let expected = "https://api.github.com/repos/wisent-ai/jeden/releases/assets/123456";
    let metadata = json!({
        "assets": [
            {"name": "other.tar.gz", "url": "https://api.github.com/repos/wisent-ai/jeden/releases/assets/7"},
            {"name": "jeden.tar.gz", "url": expected}
        ]
    });
    assert_eq!(
        super::github_asset_api_url(&metadata, "jeden.tar.gz").as_deref(),
        Some(expected)
    );
    assert_eq!(
        super::github_asset_api_url(&metadata, "missing.tar.gz"),
        None
    );

    for location in [
        "http://api.github.com/repos/wisent-ai/jeden/releases/assets/123456",
        "https://evil.api.github.com/repos/wisent-ai/jeden/releases/assets/123456",
        "https://api.github.com.evil.example/repos/wisent-ai/jeden/releases/assets/123456",
        "https://api.github.com@evil.example/repos/wisent-ai/jeden/releases/assets/123456",
        "https://api.github.com/repos/another-owner/jeden/releases/assets/123456",
        "https://api.github.com/repos/wisent-ai/another-repo/releases/assets/123456",
        "https://api.github.com/repos/wisent-ai/jeden-malicious/releases/assets/123456",
        "https://api.github.com/repos/wisent-ai/jeden/releases/assets/not-numeric",
        "https://api.github.com/repos/wisent-ai/jeden/releases/assets/123456/extra",
        "https://api.github.com/repos/wisent-ai/jeden/releases/123456",
    ] {
        let metadata = json!({
            "assets": [{"name": "jeden.tar.gz", "url": location}]
        });
        assert_eq!(
            super::github_asset_api_url(&metadata, "jeden.tar.gz"),
            None,
            "must reject untrusted asset API URL {location}"
        );
    }
}

#[test]
fn local_fetch_errors_never_format_github_tokens() {
    let _guard = UPDATE_TOKEN_ENVIRONMENT
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let _environment = UpdateTokenEnvironment::isolated();
    let secret = "do-not-format-this-secret";
    UpdateTokenEnvironment::set("JEDEN_UPDATE_GITHUB_TOKEN", Some(secret));

    let error = super::fetch(
        "/path/that/does/not/exist/jeden-private-release-manifest.json",
        1024,
    )
    .unwrap_err();
    assert!(!error.contains(secret), "fetch error leaked update token");
}

#[cfg(unix)]
mod unix_transactions {
    use super::super::run_health;
    use super::super::transaction::{self, InstallPaths};
    use std::ffi::OsString;
    use std::fs;
    use std::os::unix::fs::PermissionsExt;
    use std::path::{Path, PathBuf};
    use std::process::{Command, Stdio};
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::sync::{Arc, Barrier, Mutex};
    use std::thread;

    static NEXT_DIR: AtomicU64 = AtomicU64::new(0);
    static SERVICE_ENVIRONMENT: Mutex<()> = Mutex::new(());

    struct ServiceEnvironment(Vec<(&'static str, Option<OsString>)>);

    impl ServiceEnvironment {
        fn without_urls() -> Self {
            let values = ["BRAMA_URL", "WELES_URL"]
                .into_iter()
                .map(|name| {
                    let previous = std::env::var_os(name);
                    std::env::remove_var(name);
                    (name, previous)
                })
                .collect();
            Self(values)
        }
    }

    impl Drop for ServiceEnvironment {
        fn drop(&mut self) {
            for (name, previous) in self.0.drain(..) {
                match previous {
                    Some(value) => std::env::set_var(name, value),
                    None => std::env::remove_var(name),
                }
            }
        }
    }

    struct TempDir(PathBuf);

    impl TempDir {
        fn new(name: &str) -> Self {
            let sequence = NEXT_DIR.fetch_add(1, Ordering::Relaxed);
            let path = std::env::temp_dir().join(format!(
                "jeden-update-{name}-{}-{sequence}",
                std::process::id()
            ));
            fs::create_dir(&path).unwrap();
            Self(path)
        }

        fn path(&self) -> &Path {
            &self.0
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    fn script(exit_code: u8) -> Vec<u8> {
        format!("#!/bin/sh\nexit {exit_code}\n").into_bytes()
    }

    fn path_sensitive_candidate() -> Vec<u8> {
        b"#!/bin/sh\ncase \"$0\" in\n  *.stage) exit 0 ;;\n  *) exit 9 ;;\nesac\n".to_vec()
    }

    fn write_executable(path: &Path, bytes: &[u8]) {
        fs::write(path, bytes).unwrap();
        let mut permissions = fs::metadata(path).unwrap().permissions();
        permissions.set_mode(0o755);
        fs::set_permissions(path, permissions).unwrap();
    }

    fn shell_quote(path: &Path) -> String {
        format!("'{}'", path.to_str().unwrap().replace('\'', "'\"'\"'"))
    }

    fn run_script(path: &Path, cwd: &Path) -> Result<(), String> {
        let status = Command::new(path)
            .args(["doctor", "--json", "--cwd"])
            .arg(cwd)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .map_err(|error| error.to_string())?;
        if status.success() {
            Ok(())
        } else {
            Err(format!("script exited with {status}"))
        }
    }

    fn fixture(name: &str, old: &[u8]) -> (TempDir, InstallPaths) {
        let directory = TempDir::new(name);
        let target = directory.path().join("jeden");
        write_executable(&target, old);
        let paths = InstallPaths::new(target).unwrap();
        fs::write(
            &paths.state,
            b"{\"schemaVersion\":1,\"version\":\"1.0.0\",\"artifactSha256\":\"old-digest\"}\n",
        )
        .unwrap();
        (directory, paths)
    }

    #[test]
    fn run_health_uses_local_capabilities_without_service_configuration() {
        let _environment_lock = SERVICE_ENVIRONMENT
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let _environment = ServiceEnvironment::without_urls();
        let binary_directory = TempDir::new("health-binary");
        let owner_cwd = TempDir::new("health-owner-cwd");
        assert_eq!(fs::read_dir(owner_cwd.path()).unwrap().count(), 0);

        let binary = binary_directory.path().join("jeden");
        let invocation = binary_directory.path().join("invocation");
        let fixture = format!(
            "#!/bin/sh\nif [ \"${{BRAMA_URL+x}}\" = x ] || [ \"${{WELES_URL+x}}\" = x ]; then exit 71; fi\nprintf '%s\\n' \"$@\" > {}\n",
            shell_quote(&invocation)
        );
        write_executable(&binary, fixture.as_bytes());

        run_health(&binary, owner_cwd.path()).unwrap();

        assert_eq!(
            fs::read_to_string(invocation).unwrap(),
            format!("capabilities\n--cwd\n{}\n", owner_cwd.path().display())
        );
    }

    #[test]
    fn run_health_rejects_nonzero_capabilities_fixture() {
        let binary_directory = TempDir::new("health-nonzero");
        let owner_cwd = TempDir::new("health-nonzero-owner-cwd");
        let binary = binary_directory.path().join("jeden");
        write_executable(&binary, &script(23));

        let error = run_health(&binary, owner_cwd.path()).unwrap_err();

        assert!(
            error.contains("health probe failed with exit status: 23"),
            "{error}"
        );
    }

    #[test]
    fn run_health_rejects_timed_out_capabilities_fixture() {
        let binary_directory = TempDir::new("health-timeout");
        let owner_cwd = TempDir::new("health-timeout-owner-cwd");
        let binary = binary_directory.path().join("jeden");
        write_executable(&binary, b"#!/bin/sh\nexec sleep 60\n");

        let error = run_health(&binary, owner_cwd.path()).unwrap_err();

        assert_eq!(error, "health probe timed out");
    }

    #[test]
    fn concurrent_install_is_rejected_while_durable_lock_is_held() {
        let old = script(0);
        let new = b"#!/bin/sh\n# complete-new-release\nexit 0\n".to_vec();
        let (_directory, paths) = fixture("lock", &old);
        let worker_paths = paths.clone();
        let worker_artifact = new.clone();
        let entered_health = Arc::new(Barrier::new(2));
        let release_health = Arc::new(Barrier::new(2));
        let worker_entered = entered_health.clone();
        let worker_release = release_health.clone();

        let worker = thread::spawn(move || {
            let mut first_probe = true;
            transaction::install(
                &worker_paths,
                &worker_artifact,
                "1.0.0",
                "2.0.0",
                "new-digest",
                None,
                |binary, cwd| {
                    run_script(binary, cwd)?;
                    if first_probe {
                        first_probe = false;
                        worker_entered.wait();
                        worker_release.wait();
                    }
                    Ok(())
                },
            )
        });

        entered_health.wait();
        assert!(
            paths.lock.exists(),
            "lock must be visible before a competing updater runs"
        );
        let competing = transaction::install(
            &paths,
            b"#!/bin/sh\n# competing-release\nexit 0\n",
            "1.0.0",
            "3.0.0",
            "competing-digest",
            None,
            run_script,
        )
        .unwrap_err();
        assert!(competing.contains("durable update lock"), "{competing}");
        assert_eq!(fs::read(&paths.target).unwrap(), old);

        release_health.wait();
        worker.join().unwrap().unwrap();
        assert_eq!(fs::read(&paths.target).unwrap(), new);
        assert!(!paths.lock.exists());
    }

    #[test]
    fn every_named_crash_point_recovers_last_known_good_complete_binary() {
        let crash_points = [
            "after-prepared-journal-fsync",
            "after-stage-fsync",
            "after-backup-rename-fsync",
            "after-activate-rename-fsync",
            "after-state-fsync",
        ];

        for point in crash_points {
            let old = b"#!/bin/sh\n# last-known-good-complete\nexit 0\n".to_vec();
            let new = b"#!/bin/sh\n# candidate-complete\nexit 0\n".to_vec();
            let (_directory, paths) = fixture(point, &old);
            let error = transaction::install(
                &paths,
                &new,
                "1.0.0",
                "2.0.0",
                "candidate-digest",
                Some(point),
                run_script,
            )
            .unwrap_err();
            assert!(error.contains(point), "{point}: {error}");

            transaction::recover(&paths).unwrap();

            let recovered = fs::read(&paths.target).unwrap();
            assert!(
                recovered == old || recovered == new,
                "{point}: recovered partial or unknown bytes"
            );
            assert_eq!(recovered, old, "{point}: last-known-good was not restored");
            let state = transaction::read_installed_state(&paths).unwrap().unwrap();
            assert_eq!(
                state.version, "1.0.0",
                "{point}: recovered binary has candidate version state"
            );
            assert_eq!(
                state.artifact_sha256, "old-digest",
                "{point}: last-known-good digest state was not restored"
            );
            assert!(!paths.stage.exists(), "{point}: stage survived recovery");
            assert!(!paths.backup.exists(), "{point}: backup survived recovery");
            assert!(
                !paths.journal.exists(),
                "{point}: journal survived recovery"
            );
        }
    }

    #[test]
    fn unhealthy_current_binary_refuses_update_before_activation() {
        let old = script(7);
        let candidate = script(0);
        let (_directory, paths) = fixture("pre-health", &old);

        let error = transaction::install(
            &paths,
            &candidate,
            "1.0.0",
            "2.0.0",
            "candidate-digest",
            None,
            run_script,
        )
        .unwrap_err();

        assert!(error.contains("pre-update health failed"), "{error}");
        assert_eq!(fs::read(&paths.target).unwrap(), old);
        transaction::recover(&paths).unwrap();
        assert_eq!(fs::read(&paths.target).unwrap(), old);
    }

    #[test]
    fn unhealthy_staged_binary_refuses_update_before_activation() {
        let old = script(0);
        let candidate = script(8);
        let (_directory, paths) = fixture("staged-health", &old);

        let error = transaction::install(
            &paths,
            &candidate,
            "1.0.0",
            "2.0.0",
            "candidate-digest",
            None,
            run_script,
        )
        .unwrap_err();

        assert!(error.contains("staged update health failed"), "{error}");
        assert_eq!(fs::read(&paths.target).unwrap(), old);
        transaction::recover(&paths).unwrap();
        assert_eq!(fs::read(&paths.target).unwrap(), old);
    }

    #[test]
    fn failed_post_activation_health_restores_last_known_good_bytes() {
        let old = script(0);
        let candidate = path_sensitive_candidate();
        let (_directory, paths) = fixture("post-health", &old);

        let error = transaction::install(
            &paths,
            &candidate,
            "1.0.0",
            "2.0.0",
            "candidate-digest",
            None,
            run_script,
        )
        .unwrap_err();

        assert!(error.contains("post-update health failed"), "{error}");
        assert!(error.contains("previous binary restored"), "{error}");
        let restored = fs::read(&paths.target).unwrap();
        assert!(
            restored == old || restored == candidate,
            "rollback left partial or unknown bytes"
        );
        assert_eq!(restored, old);
        assert!(!paths.backup.exists());
        assert!(!paths.journal.exists());
    }
}
