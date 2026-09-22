//! Deciding whether an outcome already on disk may stand in for running a
//! case again.
//!
//! Split out of `eval/runner.rs`, which had grown past the module line cap.

use super::super::dataset::{safe_relative, EvalCaseV1};
use super::super::graders::sha256;
use super::super::metrics::RunOutcomeV1;
use super::{EvalRunner, IsolatedRunV1};
use std::fs;
use crate::eval::scoring::metrics::OUTCOME_SCHEMA;

impl EvalRunner {
    pub(super) fn validate_resumed(
        &self,
        outcome: &RunOutcomeV1,
        case: &EvalCaseV1,
        run_key: &str,
        fixture_digest: &str,
        grader_digest: &str,
        isolated: &IsolatedRunV1,
    ) -> Result<(), String> {
        if outcome.schema != OUTCOME_SCHEMA
            || outcome.run_key != run_key
            || outcome.case_id != case.id
            || outcome.seed != case.seed
            || outcome.dataset_digest != sha256(&self.dataset_bytes)
            || outcome.fixture_digest != fixture_digest
            || outcome.grader_digest != grader_digest
            || outcome.code_digest != self.manifest.code_digest
            || outcome.catalog_digest != self.catalog_digest
            || outcome.policy_digest != self.policy_digest
        {
            return Err(format!(
                "resumable outcome for {} does not match immutable run inputs",
                case.id
            ));
        }
        for expected in &case.expected_artifacts {
            let bytes = fs::read(isolated.artifacts.join(safe_relative(&expected.path)?)).map_err(
                |error| {
                    format!(
                        "resumable outcome missing artifact {}: {error}",
                        expected.path
                    )
                },
            )?;
            if sha256(bytes) != expected.sha256 {
                return Err(format!(
                    "resumable artifact {} digest mismatch",
                    expected.path
                ));
            }
        }
        Ok(())
    }
}
