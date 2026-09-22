//! The shape of a conformance report, and the small checks that fill it in.
//!
//! Split out of `conformance/mod.rs`, which had grown past the module line
//! cap.

use super::areas;
use super::{CHECK_PROTOCOL_VERSION, CHECK_VERSION};
use crate::capability::{self, FunctionTarget};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BehaviorEvidence {
    pub area: String,
    pub scenario: String,
    pub outcome: String,
    pub artifact: Option<String>,
    pub artifact_digest: Option<String>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CheckStatus {
    NotRun,
    Failed,
    ExternalBlocked,
    Passed,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum BehaviorCheckKind {
    Contract,
    Inventory,
    Behavior,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BehaviorAttempt {
    pub attempt: u32,
    pub started_at: u64,
    pub finished_at: u64,
    pub outcome: String,
    #[serde(default)]
    pub detail: String,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BehaviorCheckResult {
    pub id: String,
    pub kind: BehaviorCheckKind,
    pub check_version: u32,
    pub status: CheckStatus,
    pub fixture_digest: Option<String>,
    pub command_or_scenario_id: Option<String>,
    pub started_at: Option<u64>,
    pub finished_at: Option<u64>,
    pub attempts: Vec<BehaviorAttempt>,
    pub evidence_artifact_digest: Option<String>,
    pub protocol_version: String,
    pub detail: String,
}

impl BehaviorCheckResult {
    fn base(id: impl Into<String>, status: CheckStatus, detail: impl Into<String>) -> Self {
        Self {
            id: id.into(),
            kind: BehaviorCheckKind::Behavior,
            check_version: CHECK_VERSION,
            status,
            fixture_digest: None,
            command_or_scenario_id: None,
            started_at: None,
            finished_at: None,
            attempts: Vec::new(),
            evidence_artifact_digest: None,
            protocol_version: CHECK_PROTOCOL_VERSION.into(),
            detail: detail.into(),
        }
    }

    pub(crate) fn not_run(id: impl Into<String>, detail: impl Into<String>) -> Self {
        Self::base(id, CheckStatus::NotRun, detail)
    }

    pub(crate) fn failed(id: impl Into<String>, detail: impl Into<String>) -> Self {
        Self::base(id, CheckStatus::Failed, detail)
    }
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AreaResult {
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
pub struct ProductionScopeResult {
    pub id: &'static str,
    pub check_id: &'static str,
    pub owner: &'static str,
    pub artifact_path: &'static str,
    pub status: CheckStatus,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UiHonestyFinding {
    pub rule: &'static str,
    pub subject: String,
    pub detail: String,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ConformanceReport {
    pub schema_version: u32,
    pub complete: bool,
    pub area_count: usize,
    pub complete_count: usize,
    pub production_scope_count: usize,
    pub production_scopes: Vec<ProductionScopeResult>,
    pub areas: Vec<AreaResult>,
    pub ui_honesty: Vec<UiHonestyFinding>,
}

pub(super) fn contract_check(id: String, passed: bool, detail: &'static str) -> BehaviorCheckResult {
    let mut result = BehaviorCheckResult::base(
        id,
        if passed {
            CheckStatus::Passed
        } else {
            CheckStatus::Failed
        },
        detail,
    );
    result.kind = BehaviorCheckKind::Contract;
    result
}

pub(super) fn registered_checks(area: &areas::CompletionArea) -> Vec<BehaviorCheckResult> {
    let mut checks = vec![
        contract_check(
            format!("{}/metadata", area.id),
            !area.owner.trim().is_empty() && !area.acceptance.trim().is_empty(),
            "owner and measurable acceptance are registered",
        ),
        contract_check(
            format!("{}/stable-id", area.id),
            area.id
                .bytes()
                .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-'),
            "area identifier is canonical kebab-case",
        ),
    ];
    checks.sort_by(|a, b| a.id.cmp(&b.id));
    checks
}

fn executable_findings(cwd: &Path) -> Vec<UiHonestyFinding> {
    let snapshot = capability::for_cwd(cwd);
    let mut findings = Vec::new();
    for descriptor in snapshot
        .descriptors
        .iter()
        .filter(|descriptor| descriptor.ui.visible && descriptor.ui.executable)
    {
        let subject = descriptor.id.clone();
        if descriptor.operations.is_empty()
            || matches!(descriptor.target, FunctionTarget::None)
            || descriptor.binding.handler_id.trim().is_empty()
        {
            findings.push(UiHonestyFinding {
                rule: "executable-without-handler",
                subject: subject.clone(),
                detail: "descriptor does not bind an operation and concrete handler".into(),
            });
        }
        if descriptor.binding.input_schema_id.trim().is_empty()
            || descriptor.binding.output_schema_id.trim().is_empty()
        {
            findings.push(UiHonestyFinding {
                rule: "executable-without-schema",
                subject: subject.clone(),
                detail: "descriptor handler lacks input or output schema binding".into(),
            });
        }
        if !descriptor
            .binding
            .effective_grants
            .is_subset(&descriptor.binding.requested_grants)
        {
            findings.push(UiHonestyFinding {
                rule: "effective-grant-escalation",
                subject: subject.clone(),
                detail: "effective grants exceed descriptor requested grants".into(),
            });
        }
        if !descriptor.health.is_executable() || descriptor.health_evidence_id.trim().is_empty() {
            findings.push(UiHonestyFinding {
                rule: "executable-without-health",
                subject: subject.clone(),
                detail: "handler has no executable health state and evidence binding".into(),
            });
        }
        if descriptor.generation != snapshot.generation {
            findings.push(UiHonestyFinding {
                rule: "stale-capability-generation",
                subject: subject.clone(),
                detail: "surface is not pinned to the published capability generation".into(),
            });
        }
        if descriptor.ui.action.as_deref().is_none_or(str::is_empty) {
            findings.push(UiHonestyFinding {
                rule: "executable-without-surface",
                subject,
                detail: "descriptor handler health chain has no invokable surface action".into(),
            });
        }
    }
    findings
}

pub(crate) fn audit_ui_honesty_paths(cwd: &Path, _roots: &[PathBuf]) -> Vec<UiHonestyFinding> {
    let mut findings = executable_findings(cwd);
    findings
        .sort_by(|a, b| (&a.rule, &a.subject, &a.detail).cmp(&(&b.rule, &b.subject, &b.detail)));
    findings.dedup_by(|a, b| a.rule == b.rule && a.subject == b.subject && a.detail == b.detail);
    findings
}
