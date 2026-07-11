use super::{
    canonical_envelope_json, digest_bytes, key_id, markdown_from_machine_report, scan_private_data,
    sign, verify, verify_report, Aggregator, DsseEnvelope, Evidence, Metric, QualityReport, Status,
    TrustedRoot, EVIDENCE_PAYLOAD_TYPE, REPORT_PAYLOAD_TYPE,
};
use base64::{engine::general_purpose::STANDARD as BASE64, Engine as _};
use ed25519_dalek::SigningKey;
use std::collections::BTreeMap;

const REVISION: &str = "0123456789abcdef";
const ENVIRONMENT: &str = "linux-arm64-release";
const NOW: u64 = 1_800_000_000;
const MAXIMUM_AGE: u64 = 300;

fn signing_key(seed: u8) -> SigningKey {
    SigningKey::from_bytes(&[seed; 32])
}

fn trusted_root(key: &SigningKey) -> TrustedRoot {
    let verifying_key = key.verifying_key();
    TrustedRoot {
        ed25519_keys: BTreeMap::from([(
            key_id(&verifying_key),
            hex::encode(verifying_key.as_bytes()),
        )]),
    }
}

fn evidence(area: &str, status: Status) -> Evidence {
    let prerequisites = if status == Status::ExternalBlocked {
        vec!["vendor attestation".into(), "hardware lab access".into()]
    } else {
        Vec::new()
    };
    Evidence {
        schema_version: 1,
        area: area.into(),
        status,
        metrics: vec![Metric {
            name: "acceptance cases".into(),
            numerator: 12,
            denominator: 12,
        }],
        environment: ENVIRONMENT.into(),
        revision: REVISION.into(),
        evidence_uri: format!("artifact://quality/{area}"),
        evidence_digest: digest_bytes(format!("evidence:{area}").as_bytes()),
        artifact_digests: BTreeMap::from([(
            format!("{area}-results"),
            digest_bytes(format!("artifact:{area}").as_bytes()),
        )]),
        prerequisites,
        observed_at_epoch_seconds: NOW - 10,
    }
}

fn signed_evidence(evidence: &Evidence, key: &SigningKey) -> DsseEnvelope {
    sign(
        EVIDENCE_PAYLOAD_TYPE,
        &serde_json::to_vec(evidence).unwrap(),
        key,
    )
}

fn aggregator() -> Aggregator {
    Aggregator::new(REVISION, ENVIRONMENT, NOW, MAXIMUM_AGE)
}

fn aggregate(envelopes: &[DsseEnvelope]) -> (DsseEnvelope, SigningKey, TrustedRoot) {
    let evidence_key = signing_key(7);
    let report_key = signing_key(19);
    let report = aggregator()
        .aggregate(envelopes, &trusted_root(&evidence_key), &report_key)
        .unwrap();
    let report_root = trusted_root(&report_key);
    (report, report_key, report_root)
}

#[test]
fn rebuild_is_byte_identical_when_signed_evidence_arrives_in_a_different_order() {
    let evidence_key = signing_key(7);
    let alpha = signed_evidence(&evidence("alpha", Status::Passed), &evidence_key);
    let omega = signed_evidence(&evidence("omega", Status::Passed), &evidence_key);
    let report_key = signing_key(19);
    let root = trusted_root(&evidence_key);

    let forward = aggregator()
        .aggregate(&[alpha.clone(), omega.clone()], &root, &report_key)
        .unwrap();
    let reversed = aggregator()
        .aggregate(&[omega, alpha], &root, &report_key)
        .unwrap();

    assert_eq!(
        canonical_envelope_json(&forward).unwrap(),
        canonical_envelope_json(&reversed).unwrap()
    );
    let report = verify_report(&forward, &trusted_root(&report_key)).unwrap();
    assert_eq!(
        report
            .evidence
            .iter()
            .map(|item| item.area.as_str())
            .collect::<Vec<_>>(),
        ["alpha", "omega"]
    );
}

#[test]
fn report_signature_authenticates_the_report_and_rejects_payload_tampering() {
    let evidence_key = signing_key(7);
    let envelope = signed_evidence(&evidence("runtime", Status::Passed), &evidence_key);
    let (report_envelope, _, report_root) = aggregate(&[envelope]);

    let report = verify_report(&report_envelope, &report_root).unwrap();
    assert_eq!(report.revision, REVISION);
    assert_eq!(report.status, Status::Passed);

    let mut tampered = report_envelope;
    let mut payload = BASE64.decode(&tampered.payload).unwrap();
    let revision_offset = payload
        .windows(REVISION.len())
        .position(|window| window == REVISION.as_bytes())
        .unwrap();
    payload[revision_offset] ^= 1;
    tampered.payload = BASE64.encode(payload);

    assert_eq!(
        verify_report(&tampered, &report_root).unwrap_err(),
        "DSSE signature verification failed"
    );
}

#[test]
fn aggregation_rejects_revision_environment_and_freshness_mismatches() {
    let evidence_key = signing_key(7);
    let root = trusted_root(&evidence_key);
    let report_key = signing_key(19);

    let mut wrong_revision = evidence("revision", Status::Passed);
    wrong_revision.revision = "different-revision".into();
    let mut wrong_environment = evidence("environment", Status::Passed);
    wrong_environment.environment = "different-environment".into();
    let mut stale = evidence("freshness", Status::Passed);
    stale.observed_at_epoch_seconds = NOW - MAXIMUM_AGE - 1;

    for (case, input, expected) in [
        ("revision", wrong_revision, "revision mismatch for revision"),
        (
            "environment",
            wrong_environment,
            "environment mismatch for environment",
        ),
        ("stale", stale, "stale evidence for freshness"),
    ] {
        let envelope = signed_evidence(&input, &evidence_key);
        let error = aggregator()
            .aggregate(&[envelope], &root, &report_key)
            .unwrap_err();
        assert_eq!(error, expected, "{case}");
    }
}

#[test]
fn external_blocked_report_stays_blocked_and_names_its_prerequisites() {
    let evidence_key = signing_key(7);
    let blocked = signed_evidence(
        &evidence("external-certification", Status::ExternalBlocked),
        &evidence_key,
    );
    let (report_envelope, _, report_root) = aggregate(&[blocked]);
    let report = verify_report(&report_envelope, &report_root).unwrap();

    assert_eq!(report.status, Status::ExternalBlocked);
    assert_eq!(report.evidence[0].status, Status::ExternalBlocked);
    assert_eq!(
        report.evidence[0].prerequisites,
        ["hardware lab access", "vendor attestation"]
    );

    let machine = canonical_envelope_json(&report_envelope).unwrap();
    let markdown = markdown_from_machine_report(&machine, &report_root).unwrap();
    assert!(
        markdown.contains("- Status: **ExternalBlocked**"),
        "{markdown}"
    );
    assert!(
        markdown.contains("ExternalBlocked prerequisites for **external-certification**:"),
        "{markdown}"
    );
    assert!(markdown.contains("- hardware lab access"), "{markdown}");
    assert!(markdown.contains("- vendor attestation"), "{markdown}");
    assert!(!markdown.contains("- Status: **Passed**"), "{markdown}");
}

#[test]
fn conflicting_digests_for_the_same_named_artifact_fail_closed() {
    let evidence_key = signing_key(7);
    let root = trusted_root(&evidence_key);
    let report_key = signing_key(19);
    let mut first = evidence("linux", Status::Passed);
    first.artifact_digests = BTreeMap::from([("release-binary".into(), digest_bytes(b"first"))]);
    let mut second = evidence("macos", Status::Passed);
    second.artifact_digests = BTreeMap::from([("release-binary".into(), digest_bytes(b"second"))]);

    let error = aggregator()
        .aggregate(
            &[
                signed_evidence(&first, &evidence_key),
                signed_evidence(&second, &evidence_key),
            ],
            &root,
            &report_key,
        )
        .unwrap_err();

    assert_eq!(error, "artifact digest mismatch for release-binary");
}

#[test]
fn machine_report_envelope_and_payload_are_canonical_json() {
    let evidence_key = signing_key(7);
    let envelope = signed_evidence(&evidence("canonical", Status::Passed), &evidence_key);
    let (report_envelope, _, report_root) = aggregate(&[envelope]);

    let machine = canonical_envelope_json(&report_envelope).unwrap();
    assert_eq!(machine, serde_json::to_vec(&report_envelope).unwrap());
    assert_eq!(
        serde_json::from_slice::<DsseEnvelope>(&machine).unwrap(),
        report_envelope
    );

    let payload = verify(&report_envelope, &report_root).unwrap();
    let report: QualityReport = serde_json::from_slice(&payload).unwrap();
    assert_eq!(payload, serde_json::to_vec(&report).unwrap());
}

#[test]
fn markdown_conversion_accepts_only_authentic_canonical_machine_reports() {
    let evidence_key = signing_key(7);
    let envelope = signed_evidence(&evidence("markdown", Status::Passed), &evidence_key);
    let (report_envelope, report_key, report_root) = aggregate(&[envelope]);
    let machine = canonical_envelope_json(&report_envelope).unwrap();

    let markdown = markdown_from_machine_report(&machine, &report_root).unwrap();
    assert!(
        markdown.starts_with("# Jeden quality report\n\n"),
        "{markdown}"
    );
    assert!(
        markdown.contains("| markdown | Passed | acceptance cases | 12/12 |"),
        "{markdown}"
    );

    let pretty_envelope = serde_json::to_vec_pretty(&report_envelope).unwrap();
    assert_eq!(
        markdown_from_machine_report(&pretty_envelope, &report_root).unwrap_err(),
        "machine report envelope is not canonical JSON"
    );

    let mut tampered = report_envelope.clone();
    let mut tampered_payload = BASE64.decode(&tampered.payload).unwrap();
    tampered_payload[0] ^= 1;
    tampered.payload = BASE64.encode(tampered_payload);
    let tampered_machine = canonical_envelope_json(&tampered).unwrap();
    assert_eq!(
        markdown_from_machine_report(&tampered_machine, &report_root).unwrap_err(),
        "DSSE signature verification failed"
    );

    let mut noncanonical_payload = verify(&report_envelope, &report_root).unwrap();
    noncanonical_payload.push(b' ');
    let signed_noncanonical = sign(REPORT_PAYLOAD_TYPE, &noncanonical_payload, &report_key);
    let noncanonical_machine = canonical_envelope_json(&signed_noncanonical).unwrap();
    assert_eq!(
        markdown_from_machine_report(&noncanonical_machine, &report_root).unwrap_err(),
        "quality report is not canonical JSON"
    );
}

#[test]
fn privacy_scan_rejects_secrets_absolute_paths_and_host_markers_but_generated_output_is_clean() {
    for (case, bytes, marker) in [
        (
            "secret",
            b"client_secret=do-not-publish".as_slice(),
            "client_secret",
        ),
        (
            "absolute path",
            b"log at /Users/alice/work/output".as_slice(),
            "/users/",
        ),
        (
            "host",
            b"callback is localhost:8080".as_slice(),
            "localhost",
        ),
    ] {
        let error = scan_private_data(bytes).unwrap_err();
        assert_eq!(
            error,
            format!("privacy scan rejected marker {marker}"),
            "{case}"
        );
    }

    let evidence_key = signing_key(7);
    let envelope = signed_evidence(&evidence("privacy", Status::Passed), &evidence_key);
    let (report_envelope, _, report_root) = aggregate(&[envelope]);
    let machine = canonical_envelope_json(&report_envelope).unwrap();
    let markdown = markdown_from_machine_report(&machine, &report_root).unwrap();

    scan_private_data(&machine).unwrap();
    scan_private_data(markdown.as_bytes()).unwrap();
    assert!(
        markdown.contains("artifact://quality/privacy"),
        "{markdown}"
    );
}
