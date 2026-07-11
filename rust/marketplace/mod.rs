pub mod lock;
pub mod manifest;
pub mod resolver;
pub mod service;
pub mod trust;

#[cfg(test)]
mod tests {
    use super::lock::PluginLockV1;
    use super::manifest::{
        EnvelopeSignature, MarketplaceCatalogV1, MarketplaceEnvelopeV1, PluginDependency,
        PluginReleaseV1, MARKETPLACE_SCHEMA,
    };
    use super::resolver;
    use super::service::{MarketplaceService, PackageState};
    use super::trust::TrustRootV1;
    use ed25519_dalek::{Signer, SigningKey};
    use sha2::{Digest, Sha256};
    use std::collections::{BTreeMap, BTreeSet};
    use std::fs;
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicU64, Ordering};

    static SEQUENCE: AtomicU64 = AtomicU64::new(0);
    struct Temp(PathBuf);
    impl Temp {
        fn new() -> Self {
            let path = std::env::temp_dir().join(format!(
                "jeden-marketplace-{}-{}",
                std::process::id(),
                SEQUENCE.fetch_add(1, Ordering::Relaxed)
            ));
            fs::create_dir_all(&path).unwrap();
            Self(path)
        }
    }
    impl Drop for Temp {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    fn release(
        id: &str,
        version: &str,
        bytes: &[u8],
        dependencies: Vec<PluginDependency>,
    ) -> PluginReleaseV1 {
        PluginReleaseV1 {
            id: id.into(),
            version: version.into(),
            artifact_digest: hex::encode(Sha256::digest(bytes)),
            artifact_size: bytes.len() as u64,
            artifact_url: format!("fixture://{id}/{version}"),
            dependencies,
            features: BTreeSet::new(),
            platforms: BTreeSet::new(),
            entrypoint: "index.js".into(),
        }
    }
    fn dep(id: &str, requirement: &str) -> PluginDependency {
        PluginDependency {
            id: id.into(),
            requirement: requirement.into(),
            features: BTreeSet::new(),
            optional: false,
        }
    }
    fn archive() -> Vec<u8> {
        let mut bytes = Vec::new();
        {
            let mut tar = tar::Builder::new(&mut bytes);
            let content = b"export default {}";
            let mut header = tar::Header::new_gnu();
            header.set_size(content.len() as u64);
            header.set_mode(0o644);
            header.set_cksum();
            tar.append_data(&mut header, "index.js", &content[..])
                .unwrap();
            tar.finish().unwrap();
        }
        bytes
    }
    fn signed(
        releases: Vec<PluginReleaseV1>,
        sequence: u64,
    ) -> (TrustRootV1, MarketplaceEnvelopeV1) {
        let keys = [
            SigningKey::from_bytes(&[7; 32]),
            SigningKey::from_bytes(&[9; 32]),
        ];
        let root = TrustRootV1 {
            version: 1,
            threshold: 2,
            expires_at: 500,
            keys: keys
                .iter()
                .enumerate()
                .map(|(index, key)| {
                    (
                        format!("key-{index}"),
                        hex::encode(key.verifying_key().to_bytes()),
                    )
                })
                .collect(),
            revoked_keys: BTreeSet::new(),
        };
        let mut envelope = MarketplaceEnvelopeV1 {
            schema: MARKETPLACE_SCHEMA.into(),
            root_version: 1,
            catalog: MarketplaceCatalogV1 {
                catalog_id: "fixture".into(),
                sequence,
                issued_at: 1,
                expires_at: 400,
                releases,
                revoked_keys: BTreeSet::new(),
                revoked_artifacts: BTreeSet::new(),
                metadata: BTreeMap::new(),
            },
            signatures: Vec::new(),
        };
        let payload = envelope.signing_bytes().unwrap();
        envelope.signatures = keys
            .iter()
            .enumerate()
            .map(|(index, key)| EnvelopeSignature {
                key_id: format!("key-{index}"),
                signature: hex::encode(key.sign(&payload).to_bytes()),
            })
            .collect();
        (root, envelope)
    }

    #[test]
    fn threshold_tamper_and_replay_fail_closed() {
        let bytes = archive();
        let (root, mut envelope) = signed(vec![release("a", "1.0.0", &bytes, vec![])], 2);
        root.verify_catalog(&envelope, 20, Some(1)).unwrap();
        assert!(root
            .verify_catalog(&envelope, 20, Some(2))
            .unwrap_err()
            .contains("replay"));
        envelope.catalog.releases[0].version = "9.0.0".into();
        assert!(root
            .verify_catalog(&envelope, 20, Some(1))
            .unwrap_err()
            .contains("threshold"));
        envelope.signatures.pop();
        assert!(root.verify_catalog(&envelope, 20, None).is_err());
    }

    #[test]
    fn resolver_handles_diamond_deterministically_and_rejects_conflict_cycle() {
        let bytes = archive();
        let releases = vec![
            release(
                "root",
                "1.0.0",
                &bytes,
                vec![dep("left", "^1"), dep("right", "^1")],
            ),
            release("left", "1.0.0", &bytes, vec![dep("shared", ">=1,<3")]),
            release("right", "1.0.0", &bytes, vec![dep("shared", "^2")]),
            release("shared", "2.1.0", &bytes, vec![]),
            release("shared", "1.5.0", &bytes, vec![]),
        ];
        let selected = resolver::resolve(&[dep("root", "=1.0.0")], &releases, "test").unwrap();
        assert_eq!(
            selected
                .iter()
                .find(|item| item.id == "shared")
                .unwrap()
                .version,
            "2.1.0"
        );
        let first = PluginLockV1::from_resolution("fixture", 1, b"catalog", "test", &selected)
            .canonical_bytes()
            .unwrap();
        let second = PluginLockV1::from_resolution("fixture", 1, b"catalog", "test", &selected)
            .canonical_bytes()
            .unwrap();
        assert_eq!(first, second);
        let cycle = vec![
            release("a", "1.0.0", &bytes, vec![dep("b", "*")]),
            release("b", "1.0.0", &bytes, vec![dep("a", "*")]),
        ];
        assert!(resolver::resolve(&[dep("a", "*")], &cycle, "test")
            .unwrap_err()
            .contains("cycle"));
    }

    #[test]
    fn bytes_are_verified_before_activation_and_installed_is_not_active() {
        let temp = Temp::new();
        let service = MarketplaceService::new(temp.0.join(".jeden/plugins/v2"));
        let bytes = archive();
        let release = release("safe", "1.0.0", &bytes, vec![]);
        let (root, envelope) = signed(vec![release.clone()], 1);
        let mut fetches = 0;
        let error = service
            .install_and_activate(
                &root,
                &envelope,
                None,
                20,
                &[dep("safe", "*")],
                "test",
                |_| {
                    fetches += 1;
                    Ok(b"tampered".to_vec())
                },
            )
            .unwrap_err();
        assert!(error.contains("size mismatch") || error.contains("digest mismatch"));
        assert_eq!(fetches, 1);
        assert!(service.active_packages().unwrap().is_empty());
        let dev = service.dev_link("local", &temp.0, "missing.js");
        assert!(dev.is_err());
        fs::write(temp.0.join("index.js"), "ok").unwrap();
        let dev = service.dev_link("local", &temp.0, "index.js").unwrap();
        assert_eq!(dev.state, PackageState::Installed);
        assert!(service.active_packages().unwrap().is_empty());
        let active = service
            .install_and_activate(
                &root,
                &envelope,
                None,
                20,
                &[dep("safe", "*")],
                "test",
                |_| Ok(bytes.clone()),
            )
            .unwrap();
        assert_eq!(active.packages["safe"].state, PackageState::Active);
    }

    #[test]
    fn repository_fixture_signature_and_substitution_attack() {
        let fixture =
            std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/marketplace/fixtures");
        let root: TrustRootV1 =
            serde_json::from_slice(&fs::read(fixture.join("trust-root-v1.json")).unwrap()).unwrap();
        let signed: MarketplaceEnvelopeV1 = serde_json::from_slice(
            &fs::read(fixture.join("signed-empty-catalog-v1.json")).unwrap(),
        )
        .unwrap();
        let tampered: MarketplaceEnvelopeV1 =
            serde_json::from_slice(&fs::read(fixture.join("tampered-catalog-v1.json")).unwrap())
                .unwrap();
        root.verify_catalog(&signed, 20, None).unwrap();
        assert!(root
            .verify_catalog(&tampered, 20, None)
            .unwrap_err()
            .contains("threshold"));
    }
}
