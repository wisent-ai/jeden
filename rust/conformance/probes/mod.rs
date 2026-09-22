use super::{
    BehaviorAttempt, BehaviorCheckKind, BehaviorCheckResult, BehaviorEvidence, CheckStatus,
};
use crate::capability::CapabilitySnapshot;
use serde::Deserialize;
use sha2::{Digest, Sha256};
use std::fs;
use std::path::Path;

const CHECK_VERSION: u32 = 2;
const PROTOCOL_VERSION: &str = "jeden.behavior-check.v2";

pub(crate) struct SourceProbe {
    pub id: &'static str,
    pub path: &'static str,
    pub symbols: &'static [&'static str],
}

pub(crate) struct AreaProbe {
    pub area: &'static str,
    pub sources: &'static [SourceProbe],
}

mod table;

pub(crate) use table::AREA_PROBES;


#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct EvidenceArtifact {
    protocol_version: String,
    check_version: u32,
    fixture_digest: String,
    command_or_scenario_id: String,
    started_at: u64,
    finished_at: u64,
    expires_at: u64,
    attempts: Vec<BehaviorAttempt>,
    outcome: String,
}

fn valid_digest(value: &str) -> bool {
    value.len() == 64 && value.bytes().all(|byte| byte.is_ascii_hexdigit())
}

fn inventory_check(root: &Path, area: &str, probe: &SourceProbe) -> BehaviorCheckResult {
    let path = root.join(probe.path);
    let failure = match fs::read_to_string(&path) {
        Ok(source) => probe
            .symbols
            .iter()
            .find(|symbol| !source.contains(**symbol))
            .map(|symbol| format!("missing required symbol `{symbol}` in {}", path.display())),
        Err(error) => Some(format!("cannot read {}: {error}", path.display())),
    };
    BehaviorCheckResult {
        id: format!("{area}/{}/inventory", probe.id),
        kind: BehaviorCheckKind::Inventory,
        check_version: CHECK_VERSION,
        status: if failure.is_some() {
            CheckStatus::Failed
        } else {
            CheckStatus::NotRun
        },
        fixture_digest: None,
        command_or_scenario_id: None,
        started_at: None,
        finished_at: None,
        attempts: Vec::new(),
        evidence_artifact_digest: None,
        protocol_version: PROTOCOL_VERSION.into(),
        detail: failure.unwrap_or_else(|| {
            format!(
                "inventory anchors found in {}; source presence is not behavioral evidence",
                probe.path
            )
        }),
    }
}

fn evidence_check(
    root: &Path,
    area: &str,
    probe: &SourceProbe,
    now_ms: u64,
) -> (
    BehaviorCheckResult,
    Option<BehaviorEvidence>,
    Option<String>,
) {
    let id = format!("{area}/{}/behavior", probe.id);
    let relative = format!(".jeden/conformance/evidence/{area}--{}.json", probe.id);
    let path = root.join(&relative);
    let bytes = match fs::read(&path) {
        Ok(bytes) => bytes,
        Err(error) => {
            return (
                BehaviorCheckResult::not_run(
                    id,
                    format!("missing behavioral evidence {}: {error}", path.display()),
                ),
                None,
                Some(probe.id.into()),
            )
        }
    };
    let artifact_digest = hex::encode(Sha256::digest(&bytes));
    let artifact: EvidenceArtifact = match serde_json::from_slice(&bytes) {
        Ok(artifact) => artifact,
        Err(error) => {
            return (
                BehaviorCheckResult::failed(
                    id,
                    format!("invalid behavioral evidence {}: {error}", path.display()),
                ),
                None,
                Some(probe.id.into()),
            )
        }
    };
    let external_blocked = artifact.outcome == "external-blocked";
    let invalid = if artifact.protocol_version != PROTOCOL_VERSION {
        Some(format!(
            "unsupported protocol version {}",
            artifact.protocol_version
        ))
    } else if artifact.check_version != CHECK_VERSION {
        Some(format!(
            "unsupported check version {}",
            artifact.check_version
        ))
    } else if !valid_digest(&artifact.fixture_digest) {
        Some("fixture digest must be a SHA-256 hex digest".into())
    } else if artifact.command_or_scenario_id.trim().is_empty() {
        Some("command/scenario ID is empty".into())
    } else if artifact.attempts.is_empty() {
        Some("evidence contains no execution attempts".into())
    } else if artifact.started_at > artifact.finished_at
        || artifact.finished_at > artifact.expires_at
    {
        Some("evidence timestamps are inconsistent".into())
    } else if artifact.expires_at < now_ms {
        Some(format!(
            "behavioral evidence expired at {}",
            artifact.expires_at
        ))
    } else if external_blocked {
        Some("behavioral scenario is blocked by an explicit external prerequisite".into())
    } else if artifact.outcome != "passed" {
        Some(format!(
            "behavioral scenario outcome is {}",
            artifact.outcome
        ))
    } else if artifact
        .attempts
        .iter()
        .any(|attempt| attempt.outcome != "passed" || attempt.started_at > attempt.finished_at)
    {
        Some("one or more execution attempts failed or have inconsistent timestamps".into())
    } else {
        None
    };
    let result = BehaviorCheckResult {
        id,
        kind: BehaviorCheckKind::Behavior,
        check_version: artifact.check_version,
        status: if external_blocked {
            CheckStatus::ExternalBlocked
        } else if invalid.is_some() {
            CheckStatus::Failed
        } else {
            CheckStatus::Passed
        },
        fixture_digest: Some(artifact.fixture_digest.clone()),
        command_or_scenario_id: Some(artifact.command_or_scenario_id.clone()),
        started_at: Some(artifact.started_at),
        finished_at: Some(artifact.finished_at),
        attempts: artifact.attempts,
        evidence_artifact_digest: Some(artifact_digest.clone()),
        protocol_version: artifact.protocol_version,
        detail: invalid
            .clone()
            .unwrap_or_else(|| "executable behavioral evidence is valid and fresh".into()),
    };
    if let Some(reason) = invalid {
        return (result, None, Some(format!("{}: {reason}", probe.id)));
    }
    let evidence = BehaviorEvidence {
        area: area.into(),
        scenario: probe.id.into(),
        outcome: "verified".into(),
        artifact: Some(relative),
        artifact_digest: Some(artifact_digest),
    };
    (result, Some(evidence), None)
}

pub(crate) fn evaluate(
    root: &Path,
    area: &str,
    _snapshot: &CapabilitySnapshot,
    now_ms: u64,
) -> (Vec<BehaviorCheckResult>, Vec<BehaviorEvidence>, Vec<String>) {
    let Some(spec) = AREA_PROBES.iter().find(|probe| probe.area == area) else {
        return (
            vec![BehaviorCheckResult::failed(
                format!("{area}/probe-registration"),
                "no conformance probe is registered",
            )],
            Vec::new(),
            vec!["unregistered-area-probe".into()],
        );
    };
    let mut checks = Vec::new();
    let mut evidence = Vec::new();
    let mut missing = Vec::new();
    for probe in spec.sources {
        checks.push(inventory_check(root, area, probe));
        let (result, item, absent) = evidence_check(root, area, probe, now_ms);
        checks.push(result);
        if let Some(item) = item {
            evidence.push(item);
        }
        if let Some(absent) = absent {
            missing.push(absent);
        }
    }
    checks.sort_by(|a, b| a.id.cmp(&b.id));
    evidence.sort_by(|a, b| a.scenario.cmp(&b.scenario));
    missing.sort();
    (checks, evidence, missing)
}
