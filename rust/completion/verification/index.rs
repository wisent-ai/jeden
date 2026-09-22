//! Finding the receipt a piece of evidence refers to, and saying whether it
//! came from the session under review or from the reviewer's own.
//!
//! Split out of `completion/verification/mod.rs`, which had grown past the
//! module line cap.

use super::super::model::*;
use super::paths;
use serde_json::Value;
use std::collections::BTreeMap;
use std::collections::BTreeSet;
use std::path::PathBuf;

pub(super) fn canonical(path: &str) -> Result<PathBuf, String> {
    std::fs::canonicalize(crate::cli::sessions::session_dir_for(path))
        .map_err(|error| format!("cannot resolve evidence session {path}: {error}"))
}

pub(super) struct EvidenceIndex {
    pub(super) parent: PathBuf,
    pub(super) reviewer: PathBuf,
    pub(super) parent_receipts: BTreeMap<String, Value>,
    pub(super) reviewer_receipts: BTreeMap<String, Value>,
}

impl EvidenceIndex {
    pub(super) fn get(&self, reference: &EvidenceReference) -> Result<(&Value, bool), String> {
        let source = canonical(&reference.session_path)?;
        let (receipts, independent) = if source == self.reviewer {
            (&self.reviewer_receipts, true)
        } else if source == self.parent
            || self
                .parent_receipts
                .get(&reference.event_id)
                .and_then(|receipt| receipt.get("sessionPath"))
                .and_then(Value::as_str)
                .is_some_and(|path| canonical(path).is_ok_and(|path| path == source))
        {
            (&self.parent_receipts, false)
        } else {
            return Err(
                "verification cited a session outside this execution and its independent review"
                    .into(),
            );
        };
        let receipt = receipts.get(&reference.event_id).ok_or_else(|| {
            format!(
                "verification cited nonexistent tool evidence: {}",
                reference.event_id
            )
        })?;
        Ok((receipt, independent))
    }

    pub(super) fn observation(&self, reference: &EvidenceReference) -> Result<bool, String> {
        let (receipt, independent) = self.get(reference)?;
        let tool = receipt
            .get("tool")
            .and_then(Value::as_str)
            .unwrap_or_default();
        Ok(independent
            && receipt["failed"] == false
            && crate::agent::is_verification_read_tool(tool))
    }

    /// Where an accepted observation actually happened.
    pub(super) fn places(&self, reference: &EvidenceReference) -> Result<BTreeSet<String>, String> {
        self.get(reference)
            .map(|(receipt, _)| paths::touched(receipt))
    }

    /// A failed operation of the execution itself. The reviewer's own
    /// failed lookups do not count: on 2026-09-18 a review cited its own
    /// `task_evidence` miss on a made-up id as the failed operation behind a
    /// block, when the task was waiting on a value only the operator held
    /// and should have asked for it.
    pub(super) fn failure(&self, reference: &EvidenceReference) -> Result<bool, String> {
        self.get(reference)
            .map(|(receipt, independent)| !independent && receipt["failed"] == true)
    }

    /// An execution failure recorded after the given unix stamp: the one
    /// kind of failure that can follow an operator's answer.
    pub(super) fn failure_after(
        &self,
        reference: &EvidenceReference,
        stamp: &str,
    ) -> Result<bool, String> {
        let (receipt, independent) = self.get(reference)?;
        let after = receipt["timestamp"]
            .as_str()
            .and_then(|at| at.parse::<u64>().ok())
            .zip(stamp.parse::<u64>().ok())
            .is_some_and(|(at, since)| at >= since);
        Ok(!independent && receipt["failed"] == true && after)
    }
}
