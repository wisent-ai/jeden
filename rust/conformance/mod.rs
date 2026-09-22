//! Whether this build does what the product says it does, area by area.

pub(crate) mod areas;
pub mod health;
mod probes;
mod report;

use areas::{completion_areas, production_scopes};
use probes::AREA_PROBES;
use std::collections::BTreeSet;
use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

pub(crate) use report::audit_ui_honesty_paths;
use report::registered_checks;
pub use report::{
    AreaResult, BehaviorAttempt, BehaviorCheckKind, BehaviorCheckResult, BehaviorEvidence,
    CheckStatus, ConformanceReport, ProductionScopeResult, UiHonestyFinding,
};
use crate::capability;
use std::path::PathBuf;

const REPORT_SCHEMA_VERSION: u32 = 2;
const CHECK_VERSION: u32 = 2;
const CHECK_PROTOCOL_VERSION: &str = "jeden.behavior-check.v2";

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
        .min(u64::MAX as u128) as u64
}

fn run_at(cwd: &Path, evidence_now_ms: u64) -> Result<ConformanceReport, String> {
    let areas = completion_areas();
    let registered = AREA_PROBES
        .iter()
        .map(|probe| probe.area)
        .collect::<BTreeSet<_>>();
    let expected = areas.iter().map(|area| area.id).collect::<BTreeSet<_>>();
    if registered != expected {
        let missing = expected
            .difference(&registered)
            .copied()
            .collect::<Vec<_>>();
        let unknown = registered
            .difference(&expected)
            .copied()
            .collect::<Vec<_>>();
        return Err(format!(
            "conformance probe registry mismatch; missing: {}; unknown: {}",
            missing.join(", "),
            unknown.join(", ")
        ));
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
        let (probe_checks, evidence, missing_evidence) =
            probes::evaluate(&source_root, area.id, &snapshot, evidence_now_ms);
        checks.extend(probe_checks);
        checks.sort_by(|a, b| a.id.cmp(&b.id));
        let behavior_complete = checks
            .iter()
            .filter(|check| check.kind == BehaviorCheckKind::Behavior)
            .all(|check| check.status == CheckStatus::Passed)
            && checks
                .iter()
                .any(|check| check.kind == BehaviorCheckKind::Behavior);
        let contracts_pass = checks
            .iter()
            .filter(|check| check.kind == BehaviorCheckKind::Contract)
            .all(|check| check.status == CheckStatus::Passed);
        let inventory_sound = checks
            .iter()
            .filter(|check| check.kind == BehaviorCheckKind::Inventory)
            .all(|check| check.status != CheckStatus::Failed);
        let computed_complete =
            behavior_complete && contracts_pass && inventory_sound && missing_evidence.is_empty();
        results.push(AreaResult {
            id: area.id,
            title: area.title,
            phase: area.phase,
            owner: area.owner,
            acceptance: area.acceptance,
            status: if computed_complete {
                "complete"
            } else {
                "blocked"
            },
            checks,
            evidence,
            missing_evidence,
        });
    }
    results.sort_by(|a, b| a.id.cmp(b.id));
    let ui_honesty = audit_ui_honesty_paths(cwd, &[source_root.join("rust")]);
    let complete_count = results
        .iter()
        .filter(|area| area.status == "complete")
        .count();
    let mut scope_results = production_scopes()
        .iter()
        .map(|scope| ProductionScopeResult {
            id: scope.id,
            check_id: scope.check_id,
            owner: scope.owner,
            artifact_path: scope.artifact_path,
            status: CheckStatus::NotRun,
        })
        .collect::<Vec<_>>();
    scope_results.sort_by(|a, b| a.id.cmp(b.id));
    let complete = complete_count == results.len()
        && ui_honesty.is_empty()
        && scope_results
            .iter()
            .all(|scope| scope.status == CheckStatus::Passed);
    Ok(ConformanceReport {
        schema_version: REPORT_SCHEMA_VERSION,
        complete,
        area_count: results.len(),
        complete_count,
        production_scope_count: scope_results.len(),
        production_scopes: scope_results,
        areas: results,
        ui_honesty,
    })
}

pub fn run(cwd: &Path) -> Result<ConformanceReport, String> {
    run_at(cwd, now_ms())
}

pub fn canonical_json(report: &ConformanceReport) -> Result<String, String> {
    serde_json::to_string(report)
        .map(|text| text + "\n")
        .map_err(|error| error.to_string())
}
