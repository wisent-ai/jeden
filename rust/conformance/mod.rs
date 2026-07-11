pub(crate) mod areas;
pub(crate) mod health;
mod probes;

use crate::capability::{self, FunctionTarget};
use areas::completion_areas;
use probes::AREA_PROBES;
use serde::Serialize;
use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

const REPORT_SCHEMA_VERSION: u32 = 1;

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct BehaviorEvidence {
    pub area: String,
    pub scenario: String,
    pub outcome: String,
    pub artifact: Option<String>,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum CheckStatus { Passed, Failed }

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct BehaviorCheckResult {
    pub id: String,
    pub status: CheckStatus,
    pub detail: String,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct AreaResult {
    pub id: &'static str,
    pub title: &'static str,
    pub phase: &'static str,
    pub owner: &'static str,
    pub acceptance: &'static str,
    pub status: &'static str,
    pub checks: Vec<BehaviorCheckResult>,
    pub evidence: Vec<BehaviorEvidence>,
    pub missing_evidence: Vec<String>,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct UiHonestyFinding {
    pub rule: &'static str,
    pub subject: String,
    pub detail: String,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ConformanceReport {
    pub schema_version: u32,
    pub complete: bool,
    pub area_count: usize,
    pub complete_count: usize,
    pub areas: Vec<AreaResult>,
    pub ui_honesty: Vec<UiHonestyFinding>,
}


fn registered_checks(area: &areas::CompletionArea) -> Vec<BehaviorCheckResult> {
    let mut checks = vec![
        BehaviorCheckResult { id: format!("{}/metadata", area.id), status: if area.owner.trim().is_empty() || area.acceptance.trim().is_empty() { CheckStatus::Failed } else { CheckStatus::Passed }, detail: "owner and measurable acceptance are registered".into() },
        BehaviorCheckResult { id: format!("{}/stable-id", area.id), status: if area.id.bytes().all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-') { CheckStatus::Passed } else { CheckStatus::Failed }, detail: "area identifier is canonical kebab-case".into() },
    ];
    checks.sort_by(|a,b| a.id.cmp(&b.id));
    checks
}

fn executable_findings(cwd: &Path) -> Vec<UiHonestyFinding> {
    let snapshot = capability::for_cwd(cwd);
    let mut findings = Vec::new();
    for descriptor in snapshot.descriptors.iter().filter(|descriptor| descriptor.ui.visible && descriptor.ui.executable) {
        if !descriptor.health.is_executable() {
            findings.push(UiHonestyFinding { rule: "executable-without-health", subject: descriptor.id.clone(), detail: "visible executable action is not backed by healthy or degraded health".into() });
        }
        if descriptor.operations.is_empty() || matches!(descriptor.target, FunctionTarget::None) {
            findings.push(UiHonestyFinding { rule: "executable-without-handler", subject: descriptor.id.clone(), detail: "visible executable action has no registered operation or handler target".into() });
        }
        if descriptor.ui.action.as_deref().is_none_or(str::is_empty) {
            findings.push(UiHonestyFinding { rule: "executable-without-action", subject: descriptor.id.clone(), detail: "visible executable action has no invocation".into() });
        }
    }
    findings
}


pub(crate) fn audit_ui_honesty_paths(cwd: &Path, _roots: &[PathBuf]) -> Vec<UiHonestyFinding> {
    let mut findings = executable_findings(cwd);
    findings.sort_by(|a,b| (&a.rule,&a.subject,&a.detail).cmp(&(&b.rule,&b.subject,&b.detail)));
    findings.dedup_by(|a,b| a.rule == b.rule && a.subject == b.subject && a.detail == b.detail);
    findings
}

pub(crate) fn run(cwd: &Path) -> Result<ConformanceReport, String> {
    let areas = completion_areas();
    let registered = AREA_PROBES.iter().map(|probe| probe.area).collect::<BTreeSet<_>>();
    let expected = areas.iter().map(|area| area.id).collect::<BTreeSet<_>>();
    if registered != expected {
        let missing = expected.difference(&registered).copied().collect::<Vec<_>>();
        let unknown = registered.difference(&expected).copied().collect::<Vec<_>>();
        return Err(format!("conformance probe registry mismatch; missing: {}; unknown: {}", missing.join(", "), unknown.join(", ")));
    }
    let source_root = if cwd.join("rust").is_dir() {
        cwd.to_path_buf()
    } else {
        PathBuf::from(env!("CARGO_MANIFEST_DIR"))
    };
    let snapshot = capability::for_cwd(cwd);
    let mut results = Vec::with_capacity(areas.len());
    for area in areas {
        let mut checks = registered_checks(area);
        let (probe_checks, evidence, missing_evidence) = probes::evaluate(&source_root, area.id, &snapshot);
        checks.extend(probe_checks);
        checks.sort_by(|a,b| a.id.cmp(&b.id));
        let checks_pass = checks.iter().all(|check| matches!(check.status, CheckStatus::Passed));
        let computed_complete = checks_pass && missing_evidence.is_empty();
        results.push(AreaResult {
            id: area.id, title: area.title, phase: area.phase, owner: area.owner, acceptance: area.acceptance,
            status: if computed_complete { "complete" } else { "blocked" }, checks, evidence, missing_evidence,
        });
    }
    results.sort_by(|a,b| a.id.cmp(b.id));
    let roots = [source_root.join("rust")];
    let ui_honesty = audit_ui_honesty_paths(cwd, &roots);
    let complete_count = results.iter().filter(|area| area.status == "complete").count();
    let complete = complete_count == results.len() && ui_honesty.is_empty();
    Ok(ConformanceReport { schema_version: REPORT_SCHEMA_VERSION, complete, area_count: results.len(), complete_count, areas: results, ui_honesty })
}

pub(crate) fn canonical_json(report: &ConformanceReport) -> Result<String, String> {
    serde_json::to_string(report).map(|text| text + "\n").map_err(|error| error.to_string())
}


#[cfg(test)]
mod tests {
    use super::*;
    use crate::capability::{CapabilityDescriptor, CapabilityKind};

    struct PublishedSnapshotGuard {
        cwd: PathBuf,
        descriptors: Vec<CapabilityDescriptor>,
    }

    impl PublishedSnapshotGuard {
        fn capture() -> Self {
            let snapshot = capability::snapshot();
            Self { cwd: snapshot.cwd.clone(), descriptors: snapshot.descriptors.iter().cloned().collect() }
        }
    }

    impl Drop for PublishedSnapshotGuard {
        fn drop(&mut self) {
            let _ = capability::publish_for_test(&self.cwd, std::mem::take(&mut self.descriptors));
        }
    }

    #[test]
    fn computed_conformance_ui_honesty_rejects_visible_executable_without_handler() {
        let _restore = PublishedSnapshotGuard::capture();
        let cwd = Path::new(env!("CARGO_MANIFEST_DIR"));
        let descriptor = CapabilityDescriptor::new(
            "view/dishonest-test-action",
            CapabilityKind::View,
            "conformance-test",
            "Dishonest action",
            "Visible executable fixture without a handler target",
            FunctionTarget::None,
        )
        .operation("run")
        .executable("dishonest-test-action");
        capability::publish_for_test(cwd, vec![descriptor]).expect("publish isolated capability fixture");

        let findings = audit_ui_honesty_paths(cwd, &[]);

        assert_eq!(findings.len(), 1);
        assert_eq!(findings[0].rule, "executable-without-handler");
        assert_eq!(findings[0].subject, "view/dishonest-test-action");
        assert_eq!(findings[0].detail, "visible executable action has no registered operation or handler target");
    }
}
