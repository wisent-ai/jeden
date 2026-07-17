mod aggregate;
mod sign;

pub use aggregate::{
    canonical_envelope_json, digest_bytes, scan_private_data, verify_report, Aggregator, Evidence,
    EvidenceSummary, Metric, QualityReport, Status, EVIDENCE_PAYLOAD_TYPE, REPORT_PAYLOAD_TYPE,
};
pub use sign::{envelope_digest, key_id, sign, verify, DsseEnvelope, DsseSignature, TrustedRoot};

use std::fmt::Write as _;

pub fn markdown_from_machine_report(
    machine_report: &[u8],
    report_root: &TrustedRoot,
) -> Result<String, String> {
    let envelope: DsseEnvelope = serde_json::from_slice(machine_report)
        .map_err(|error| format!("invalid machine report envelope: {error}"))?;
    if canonical_envelope_json(&envelope)? != machine_report {
        return Err("machine report envelope is not canonical JSON".into());
    }
    let report = verify_report(&envelope, report_root)?;
    let mut markdown = String::new();
    writeln!(markdown, "# Jeden quality report").unwrap();
    writeln!(markdown).unwrap();
    writeln!(
        markdown,
        "- Revision: `{}`",
        markdown_text(&report.revision)
    )
    .unwrap();
    writeln!(
        markdown,
        "- Environment: `{}`",
        markdown_text(&report.environment)
    )
    .unwrap();
    writeln!(markdown, "- Status: **{:?}**", report.status).unwrap();
    writeln!(
        markdown,
        "- Generated: `{}` (Unix epoch seconds)",
        report.generated_at_epoch_seconds
    )
    .unwrap();
    writeln!(markdown).unwrap();
    writeln!(
        markdown,
        "| Area | Status | Metric | Result | Evidence | Digest |"
    )
    .unwrap();
    writeln!(markdown, "|---|---|---|---:|---|---|").unwrap();
    for evidence in &report.evidence {
        for metric in &evidence.metrics {
            writeln!(
                markdown,
                "| {} | {:?} | {} | {}/{} | `{}` | `{}` |",
                markdown_text(&evidence.area),
                evidence.status,
                markdown_text(&metric.name),
                metric.numerator,
                metric.denominator,
                markdown_text(&evidence.evidence_uri),
                markdown_text(&evidence.evidence_digest),
            )
            .unwrap();
        }
        if evidence.status == Status::ExternalBlocked {
            writeln!(markdown).unwrap();
            writeln!(
                markdown,
                "ExternalBlocked prerequisites for **{}**:",
                markdown_text(&evidence.area)
            )
            .unwrap();
            for prerequisite in &evidence.prerequisites {
                writeln!(markdown, "- {}", markdown_text(prerequisite)).unwrap();
            }
        }
    }
    if !report.artifact_digests.is_empty() {
        writeln!(markdown).unwrap();
        writeln!(markdown, "## Bound artifact digests").unwrap();
        writeln!(markdown).unwrap();
        for (name, digest) in &report.artifact_digests {
            writeln!(markdown, "- `{}`: `{}`", markdown_text(name), digest).unwrap();
        }
    }
    scan_private_data(markdown.as_bytes())?;
    Ok(markdown)
}

fn markdown_text(value: &str) -> String {
    value.replace('|', "\\|").replace(['\r', '\n'], " ")
}
