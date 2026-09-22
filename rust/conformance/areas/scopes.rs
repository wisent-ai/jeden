//! The production scopes a release has to satisfy, each with the check that
//! decides it, the team that owns it and where its evidence is written.
//!
//! Split out of `conformance/areas.rs`, which had grown past the module line
//! cap.

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct ProductionScope {
    pub(crate) id: &'static str,
    pub(crate) check_id: &'static str,
    pub(crate) owner: &'static str,
    pub(crate) artifact_path: &'static str,
}

macro_rules! scope {
    ($id:literal, $owner:literal) => {
        ProductionScope {
            id: $id,
            check_id: concat!("production/", $id, "/behavior"),
            owner: $owner,
            artifact_path: concat!(".jeden/conformance/artifacts/", $id, ".json"),
        }
    };
}

pub(crate) static PRODUCTION_SCOPES: [ProductionScope; 23] = [
    scope!("01-realne-brama-weles-e2e", "control-plane"),
    scope!("02-podpisany-release-engineering", "release"),
    scope!("03-kontraktowe-ci-i-migracje", "migrations"),
    scope!("04-sandbox-i-security-audit", "runtime-security"),
    scope!("05-dlugotrwale-reliability-tests", "reliability"),
    scope!("06-private-opentelemetry", "telemetry"),
    scope!("07-coding-agent-benchmark", "quality"),
    scope!("08-outcome-based-routing", "routing"),
    scope!("09-semantic-quality-memory", "memory"),
    scope!("10-ide-acp-integration", "protocol"),
    scope!("11-stable-rust-typescript-python-sdk", "sdk"),
    scope!("12-secure-headless-service", "headless"),
    scope!("13-production-signed-marketplace", "marketplace"),
    scope!("14-remote-worker-pool", "workers"),
    scope!("15-multiplatform", "platform"),
    scope!("16-staging-brama-weles", "control-plane"),
    scope!("17-nightly-all-interface-e2e", "e2e"),
    scope!("18-crash-fault-matrix", "reliability"),
    scope!("19-conformance-ci-gate", "conformance"),
    scope!("20-signed-canary-rollback", "release"),
    scope!("21-representative-benchmark-run", "quality"),
    scope!("22-warning-debt-deny-warnings", "quality"),
    scope!("23-quality-reliability-report", "release"),
];

pub(crate) fn production_scopes() -> &'static [ProductionScope] {
    &PRODUCTION_SCOPES
}
