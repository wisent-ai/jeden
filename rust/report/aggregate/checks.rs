//! What a piece of evidence has to satisfy before it is allowed into a
//! quality report.
//!
//! Split out of `report/aggregate.rs`, which had grown past the module line
//! cap.

use super::{Aggregator, Evidence};
use crate::report::aggregate::Status;
use std::collections::BTreeSet;

pub(super) fn validate_evidence(
    aggregator: &Aggregator,
    evidence: &Evidence,
) -> Result<(), String> {
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
