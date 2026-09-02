use super::sign::{canonical_json, envelope_digest, sign, verify, DsseEnvelope, TrustedRoot};
use ed25519_dalek::SigningKey;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};

pub const EVIDENCE_PAYLOAD_TYPE: &str = "application/vnd.jeden.quality-evidence.v1+json";
pub const REPORT_PAYLOAD_TYPE: &str = "application/vnd.jeden.quality-report.v1+json";

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub enum Status {
    Passed,
    Failed,
    ExternalBlocked,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Metric {
    pub name: String,
    pub numerator: u64,
    pub denominator: u64,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Evidence {
    pub schema_version: u32,
    pub area: String,
    pub status: Status,
    pub metrics: Vec<Metric>,
    pub environment: String,
    pub revision: String,
    pub evidence_uri: String,
    pub evidence_digest: String,
    pub artifact_digests: BTreeMap<String, String>,
    pub prerequisites: Vec<String>,
    pub observed_at_epoch_seconds: u64,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct EvidenceSummary {
    pub area: String,
    pub status: Status,
    pub metrics: Vec<Metric>,
    pub environment: String,
    pub revision: String,
    pub evidence_uri: String,
    pub evidence_digest: String,
    pub source_envelope_digest: String,
    pub prerequisites: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct QualityReport {
    pub schema_version: u32,
    pub revision: String,
    pub environment: String,
    pub generated_at_epoch_seconds: u64,
    pub status: Status,
    pub evidence: Vec<EvidenceSummary>,
    pub artifact_digests: BTreeMap<String, String>,
}

pub struct Aggregator {
    revision: String,
    environment: String,
    now_epoch_seconds: u64,
    maximum_evidence_age_seconds: u64,
}

impl Aggregator {
    pub fn new(
        revision: impl Into<String>,
        environment: impl Into<String>,
        now_epoch_seconds: u64,
        maximum_evidence_age_seconds: u64,
    ) -> Self {
        Self {
            revision: revision.into(),
            environment: environment.into(),
            now_epoch_seconds,
            maximum_evidence_age_seconds,
        }
    }

    pub fn aggregate(
        &self,
        envelopes: &[DsseEnvelope],
        evidence_root: &TrustedRoot,
        report_key: &SigningKey,
    ) -> Result<DsseEnvelope, String> {
        if envelopes.is_empty() {
            return Err("quality report requires signed evidence".into());
        }
        let mut summaries = Vec::with_capacity(envelopes.len());
        let mut areas = BTreeSet::new();
        let mut artifact_digests = BTreeMap::new();

        for envelope in envelopes {
            if envelope.payload_type != EVIDENCE_PAYLOAD_TYPE {
                return Err(format!(
                    "unexpected evidence payload type {}",
                    envelope.payload_type
                ));
            }
            let payload = verify(envelope, evidence_root)?;
            let mut evidence: Evidence = serde_json::from_slice(&payload)
                .map_err(|error| format!("invalid evidence JSON: {error}"))?;
            if canonical_json(&evidence)? != payload {
                return Err(format!("evidence {} is not canonical JSON", evidence.area));
            }
            validate_evidence(self, &evidence)?;
            if !areas.insert(evidence.area.clone()) {
                return Err(format!("duplicate evidence area {}", evidence.area));
            }
            for (name, digest) in &evidence.artifact_digests {
                match artifact_digests.get(name) {
                    Some(existing) if existing != digest => {
                        return Err(format!("artifact digest mismatch for {name}"));
                    }
                    None => {
                        artifact_digests.insert(name.clone(), digest.clone());
                    }
                    _ => {}
                }
            }
            evidence
                .metrics
                .sort_by(|left, right| left.name.cmp(&right.name));
            evidence.prerequisites.sort();
            summaries.push(EvidenceSummary {
                area: evidence.area,
                status: evidence.status,
                metrics: evidence.metrics,
                environment: evidence.environment,
                revision: evidence.revision,
                evidence_uri: evidence.evidence_uri,
                evidence_digest: evidence.evidence_digest,
                source_envelope_digest: envelope_digest(envelope)?,
                prerequisites: evidence.prerequisites,
            });
        }
        summaries.sort_by(|left, right| left.area.cmp(&right.area));
        let status = if summaries.iter().any(|item| item.status == Status::Failed) {
            Status::Failed
        } else if summaries
            .iter()
            .any(|item| item.status == Status::ExternalBlocked)
        {
            Status::ExternalBlocked
        } else {
            Status::Passed
        };
        let report = QualityReport {
            schema_version: 1,
            revision: self.revision.clone(),
            environment: self.environment.clone(),
            generated_at_epoch_seconds: self.now_epoch_seconds,
            status,
            evidence: summaries,
            artifact_digests,
        };
        let payload = canonical_json(&report)?;
        scan_private_data(&payload)?;
        Ok(sign(REPORT_PAYLOAD_TYPE, &payload, report_key))
    }
}

fn validate_evidence(aggregator: &Aggregator, evidence: &Evidence) -> Result<(), String> {
    if evidence.schema_version != 1 {
        return Err(format!("unsupported evidence schema for {}", evidence.area));
    }
    if evidence.area.is_empty() || evidence.environment.is_empty() || evidence.revision.is_empty() {
        return Err("evidence area, environment, and revision are required".into());
    }
    if evidence.environment != aggregator.environment {
        return Err(format!("environment mismatch for {}", evidence.area));
    }
    if evidence.revision != aggregator.revision {
        return Err(format!("revision mismatch for {}", evidence.area));
    }
    if evidence.observed_at_epoch_seconds > aggregator.now_epoch_seconds
        || aggregator.now_epoch_seconds - evidence.observed_at_epoch_seconds
            > aggregator.maximum_evidence_age_seconds
    {
        return Err(format!("stale evidence for {}", evidence.area));
    }
    validate_digest(&evidence.evidence_digest)?;
    if !evidence.evidence_uri.starts_with("artifact://") {
        return Err(format!(
            "evidence URI for {} must be an immutable artifact URI",
            evidence.area
        ));
    }
    for digest in evidence.artifact_digests.values() {
        validate_digest(digest)?;
    }
    if evidence.metrics.is_empty() {
        return Err(format!("evidence {} has no metrics", evidence.area));
    }
    let mut metric_names = BTreeSet::new();
    for metric in &evidence.metrics {
        if metric.name.is_empty() || !metric_names.insert(&metric.name) {
            return Err(format!(
                "evidence {} has invalid metric names",
                evidence.area
            ));
        }
        if metric.denominator == 0 || metric.numerator > metric.denominator {
            return Err(format!("invalid numerator/denominator for {}", metric.name));
        }
    }
    match evidence.status {
        Status::ExternalBlocked if evidence.prerequisites.is_empty() => {
            return Err(format!(
                "ExternalBlocked evidence {} requires prerequisites",
                evidence.area
            ));
        }
        Status::Passed | Status::Failed if !evidence.prerequisites.is_empty() => {
            return Err("only ExternalBlocked evidence may list prerequisites".to_string());
        }
        _ => {}
    }
    Ok(())
}

fn validate_digest(value: &str) -> Result<(), String> {
    let hex = value
        .strip_prefix("sha256:")
        .ok_or_else(|| "digest must use sha256".to_string())?;
    if hex.len() != 64
        || !hex
            .bytes()
            .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
    {
        return Err("digest must be lowercase SHA-256 hex".into());
    }
    Ok(())
}

pub fn verify_report(envelope: &DsseEnvelope, root: &TrustedRoot) -> Result<QualityReport, String> {
    if envelope.payload_type != REPORT_PAYLOAD_TYPE {
        return Err("unexpected quality report payload type".into());
    }
    let payload = verify(envelope, root)?;
    scan_private_data(&payload)?;
    let report: QualityReport = serde_json::from_slice(&payload)
        .map_err(|error| format!("invalid quality report JSON: {error}"))?;
    if canonical_json(&report)? != payload {
        return Err("quality report is not canonical JSON".into());
    }
    Ok(report)
}

pub fn canonical_envelope_json(envelope: &DsseEnvelope) -> Result<Vec<u8>, String> {
    canonical_json(envelope)
}

pub fn scan_private_data(bytes: &[u8]) -> Result<(), String> {
    let text = std::str::from_utf8(bytes).map_err(|_| "report is not UTF-8".to_string())?;
    let lower = text.to_ascii_lowercase();
    let forbidden = [
        "-----begin ",
        "authorization:",
        "bearer ",
        "api_key",
        "api-key",
        "client_secret",
        "access_token",
        "private_key",
        "password=",
        "token=",
        "secret=",
        "file://",
        "http://",
        "https://",
        "localhost",
        "127.0.0.1",
        "\\users\\",
        "/users/",
        "/home/",
        "/tmp/",
    ];
    if let Some(marker) = forbidden.iter().find(|marker| lower.contains(**marker)) {
        return Err(format!("privacy scan rejected marker {marker}"));
    }
    let structural_markers = [
        (
            r"(?i)\b(?:[a-z0-9-]+\.)+(?:app|cloud|com|dev|internal|io|local|net|org)\b",
            "hostname",
        ),
        (r"\b(?:[0-9]{1,3}\.){3}[0-9]{1,3}\b", "IP address"),
        (
            r#"(?:^|[\s\"'])/(?:etc|opt|private|srv|var|volumes)/"#,
            "absolute path",
        ),
        (r"(?i)\b[a-z]:\\", "absolute Windows path"),
    ];
    for (pattern, name) in structural_markers {
        let regex = regex::Regex::new(pattern).map_err(|error| error.to_string())?;
        if regex.is_match(text) {
            return Err(format!("privacy scan rejected {name}"));
        }
    }
    Ok(())
}

pub fn digest_bytes(bytes: &[u8]) -> String {
    format!("sha256:{}", hex::encode(Sha256::digest(bytes)))
}
