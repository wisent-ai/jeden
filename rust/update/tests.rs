use super::manifest::{self, ReleaseManifestV2, TrustRoot, PAYLOAD_TYPE};
use base64::Engine;
use ed25519_dalek::{Signer, SigningKey};
use semver::Version;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;

const NOW: u64 = 1_800_000_000;
const TARGET: &str = "aarch64-apple-darwin";

fn signing_key() -> SigningKey {
    SigningKey::generate(&mut rand::rngs::OsRng)
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
fn accepts_canonical_dsse_ed25519_manifest_from_public_test_root() {
    let key = signing_key();
    let expected = manifest();

    let verified = verify(&expected, &key).unwrap();

    assert_eq!(verified, expected);
}

#[test]
fn rejects_payload_tampering_after_signature() {
    let key = signing_key();
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
    let signer = signing_key();
    let trusted = signing_key();
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
    let key = signing_key();
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
    let key = signing_key();
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
    let key = signing_key();
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

#[cfg(unix)]
mod unix_transactions {
    use super::super::transaction::{self, InstallPaths};
    use std::fs;
    use std::os::unix::fs::PermissionsExt;
    use std::path::{Path, PathBuf};
    use std::process::{Command, Stdio};
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::sync::{Arc, Barrier};
    use std::thread;

    static NEXT_DIR: AtomicU64 = AtomicU64::new(0);

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
