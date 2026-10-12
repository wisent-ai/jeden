//! Whether a published quality report can carry anything private, read from
//! the report's structure instead of searched for words.
//!
//! Every string a report holds is one of three things. A digest, which
//! `checks::validate_evidence` already holds to lowercase SHA-256. The
//! evidence address, `artifact://` followed by segments. Or a value the
//! evidence producer chose: the revision, the environment, an area, a metric
//! name, an artifact name (labels) and the prerequisites of a blocked area
//! (prose). A label is one run of characters with no whitespace; prose may
//! hold words. Neither may hold a character that spells a location or a
//! secret's shape — a path separator, a URL scheme or port colon, a mailbox
//! `@`, an assignment `=` — nor a word the standard library parses as an IP
//! address. The marker list this replaces (`password=`, `/home/`,
//! `localhost`, nine domain endings, …) refused those spellings and let every
//! other one through.

use std::net::IpAddr;

use super::QualityReport;

/// The characters that make a value a path, an address or an assignment.
const LOCATION_CHARACTERS: &[char] = &['/', '\\', ':', '@', '='];
/// How an evidence address begins (the evidence schema's `evidence_uri`).
const ARTIFACT_SCHEME: &str = "artifact://";
/// Separates the segments of an evidence address.
const SEGMENT_SEPARATOR: char = '/';

/// The refusal for the first value in `report` that could carry a location
/// or a secret. The value itself is never quoted: the refusal names the field.
pub fn scan_private_data(report: &QualityReport) -> Result<(), String> {
    label("revision", &report.revision)?;
    label("environment", &report.environment)?;
    for name in report.artifact_digests.keys() {
        label("artifact name", name)?;
    }
    for evidence in &report.evidence {
        label("area", &evidence.area)?;
        label("environment", &evidence.environment)?;
        label("revision", &evidence.revision)?;
        for metric in &evidence.metrics {
            label("metric name", &metric.name)?;
        }
        for prerequisite in &evidence.prerequisites {
            prose("prerequisite", prerequisite)?;
        }
        let path = evidence
            .evidence_uri
            .strip_prefix(ARTIFACT_SCHEME)
            .ok_or_else(|| {
                "privacy scan rejected an evidence address outside artifact://".to_string()
            })?;
        for segment in path.split(SEGMENT_SEPARATOR) {
            label("evidence address segment", segment)?;
        }
    }
    Ok(())
}

/// One word with nothing that spells a location.
fn label(field: &str, value: &str) -> Result<(), String> {
    if value.is_empty() || value.chars().any(char::is_whitespace) {
        return Err(format!(
            "privacy scan rejected the {field}: a label is one word"
        ));
    }
    prose(field, value)
}

/// Words with nothing that spells a location or an IP address.
fn prose(field: &str, value: &str) -> Result<(), String> {
    if value.contains(LOCATION_CHARACTERS) {
        return Err(format!(
            "privacy scan rejected the {field}: it holds a path, address or assignment character"
        ));
    }
    if value
        .split_whitespace()
        .any(|word| word.parse::<IpAddr>().is_ok())
    {
        return Err(format!(
            "privacy scan rejected the {field}: it holds an IP address"
        ));
    }
    Ok(())
}
