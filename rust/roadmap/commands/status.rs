//! Moving an item to a new state, with the rules that make the state mean
//! something.
//!
//! Split out of `roadmap/mod.rs`, which had grown past the module line cap.

use super::super::model::{RoadmapError, RoadmapStatus};
use super::super::normalize::{actor, find_item_mut, now};
use super::super::store::RoadmapStore;
use super::options::{expected_revision, format_json, ParsedOptions};
use crate::roadmap::model::EvidenceLink;
use crate::roadmap::normalize::find_item;
use serde_json::json;

pub(super) fn mutate_status(
    store: &RoadmapStore,
    options: &ParsedOptions,
    status: RoadmapStatus,
    reject_dependents: bool,
    json_output: bool,
) -> Result<String, RoadmapError> {
    let id = options
        .positionals
        .first()
        .ok_or_else(|| RoadmapError::Usage(format!("Usage: roadmap {} <id>", status.as_str())))?;
    mutate_status_explicit(
        store,
        options,
        id,
        status,
        reject_dependents,
        1,
        json_output,
    )
}

pub(super) fn mutate_status_explicit(
    store: &RoadmapStore,
    options: &ParsedOptions,
    id: &str,
    status: RoadmapStatus,
    reject_dependents: bool,
    reason_start: usize,
    json_output: bool,
) -> Result<String, RoadmapError> {
    let revision = expected_revision(store, options)?;
    let positional_reason = options
        .positionals
        .get(reason_start..)
        .unwrap_or_default()
        .join(" ");
    let reason = options.one("reason").map(str::to_string).or_else(|| {
        (!positional_reason.trim().is_empty()).then(|| positional_reason.trim().to_string())
    });
    let mut prerequisites = options.many("external-prerequisite");
    if status == RoadmapStatus::ExternalBlocked && prerequisites.is_empty() {
        if let Some(reason) = reason.as_ref() {
            prerequisites.push(reason.clone());
        }
    }
    if status == RoadmapStatus::ExternalBlocked && prerequisites.is_empty() {
        return Err(RoadmapError::Usage(
            "roadmap block requires a reason or --external-prerequisite".into(),
        ));
    }
    let evidence_uris = options.many("evidence");
    let evidence_added_at = now();
    let evidence_added_by = actor();
    let event_type = if status == RoadmapStatus::Passed {
        "roadmap_item_passed"
    } else if status == RoadmapStatus::ExternalBlocked {
        "roadmap_item_blocked"
    } else if status == RoadmapStatus::Dropped {
        "roadmap_item_dropped"
    } else {
        "roadmap_item_updated"
    };
    let roadmap = store.mutate(
        revision,
        event_type,
        json!({
            "itemId": id,
            "status": status.as_str(),
            "reason": reason.clone(),
            "externalPrerequisites": prerequisites.clone(),
            "evidence": evidence_uris.clone()
        }),
        |roadmap| {
            if reject_dependents {
                let dependents = roadmap
                    .items
                    .iter()
                    .filter(|item| {
                        item.depends_on
                            .iter()
                            .any(|value| value.eq_ignore_ascii_case(id))
                    })
                    .map(|item| item.id.clone())
                    .collect::<Vec<_>>();
                if !dependents.is_empty() {
                    return Err(RoadmapError::Invalid(format!(
                        "cannot drop {id}; depended on by {}",
                        dependents.join(", ")
                    )));
                }
            }
            let item = find_item_mut(roadmap, id)?;
            item.status = status;
            item.reason = reason;
            for prerequisite in prerequisites {
                if !item.external_prerequisites.contains(&prerequisite) {
                    item.external_prerequisites.push(prerequisite);
                }
            }
            for uri in evidence_uris {
                item.evidence.push(EvidenceLink {
                    uri,
                    acceptance_id: None,
                    added_at: evidence_added_at.clone(),
                    added_by: evidence_added_by.clone(),
                });
            }
            Ok(())
        },
    )?;
    if json_output {
        format_json(find_item(&roadmap, id)?)
    } else {
        Ok(format!(
            "Updated {} at roadmap revision {}.\n",
            id, roadmap.revision
        ))
    }
}
