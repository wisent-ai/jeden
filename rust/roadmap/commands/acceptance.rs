//! The acceptance criteria of a roadmap item, and the evidence attached to
//! them.
//!
//! Split out of `roadmap/mod.rs`, which had grown past the module line cap.

use super::super::model::{AcceptanceCriterion, EvidenceLink, RoadmapError, RoadmapStatus};
use super::super::normalize::{actor, find_item, find_item_mut, next_number, now};
use super::super::store::RoadmapStore;
use super::options::{expected_revision, format_json, ParsedOptions};
use serde_json::json;

pub(super) fn acceptance_command(
    store: &RoadmapStore,
    options: &ParsedOptions,
    json_output: bool,
) -> Result<String, RoadmapError> {
    let [operation, item_id, rest @ ..] = options.positionals.as_slice() else {
        return Err(RoadmapError::Usage(
            "Usage: roadmap acceptance <list|add|evidence> <item-id> ...".into(),
        ));
    };
    if operation == "list" {
        let roadmap = store.load()?;
        let item = find_item(&roadmap, item_id)?;
        if json_output {
            return format_json(&json!({
                "itemId": item.id,
                "acceptance": item.acceptance,
                "evidence": item.evidence,
            }));
        }
        let mut output = format!("Acceptance for {}\n", item.id);
        for criterion in &item.acceptance {
            let evidence = item
                .evidence
                .iter()
                .filter(|entry| entry.acceptance_id.as_deref() == Some(&criterion.id))
                .count();
            output.push_str(&format!(
                "{}\t{}\t{} evidence\n",
                criterion.id, criterion.text, evidence
            ));
        }
        return Ok(output);
    }
    let revision = expected_revision(store, options)?;
    match operation.as_str() {
        "add" => {
            let text = rest.join(" ");
            if text.trim().is_empty() {
                return Err(RoadmapError::Usage(
                    "Usage: roadmap acceptance add <item-id> <criterion>".into(),
                ));
            }
            let mut criterion_id = String::new();
            let roadmap = store.mutate(
                revision,
                "roadmap_item_updated",
                json!({"itemId": item_id, "operation": "add", "text": text}),
                |roadmap| {
                    let item = find_item_mut(roadmap, item_id)?;
                    criterion_id = match options.one("id") {
                        Some(named) => named.to_string(),
                        None => format!(
                            "acceptance-{}",
                            next_number(
                                item.acceptance
                                    .iter()
                                    .map(|criterion| criterion.id.as_str()),
                                "acceptance-",
                            )?
                        ),
                    };
                    item.acceptance.push(AcceptanceCriterion {
                        id: criterion_id.clone(),
                        text,
                    });
                    Ok(())
                },
            )?;
            if json_output {
                format_json(find_item(&roadmap, item_id)?)
            } else {
                Ok(format!(
                    "Added {} to {} at roadmap revision {}.\n",
                    criterion_id, item_id, roadmap.revision
                ))
            }
        }
        "evidence" => {
            let [criterion_id, uri, ..] = rest else {
                return Err(RoadmapError::Usage(
                    "Usage: roadmap acceptance evidence <item-id> <acceptance-id> <artifact-uri>"
                        .into(),
                ));
            };
            let added_at = now()?;
            let roadmap = store.mutate(
                revision,
                "roadmap_evidence_attached",
                json!({"itemId": item_id, "acceptanceId": criterion_id, "uri": uri}),
                |roadmap| {
                    let item = find_item_mut(roadmap, item_id)?;
                    if !item
                        .acceptance
                        .iter()
                        .any(|criterion| criterion.id == *criterion_id)
                    {
                        return Err(RoadmapError::Invalid(format!(
                            "{} has no acceptance criterion {}",
                            item.id, criterion_id
                        )));
                    }
                    item.evidence.push(EvidenceLink {
                        uri: uri.clone(),
                        acceptance_id: Some(criterion_id.clone()),
                        added_at: added_at.clone(),
                        added_by: actor()?,
                    });
                    Ok(())
                },
            )?;
            if json_output {
                format_json(find_item(&roadmap, item_id)?)
            } else {
                Ok(format!(
                    "Attached evidence to {} at roadmap revision {}.\n",
                    item_id, roadmap.revision
                ))
            }
        }
        other => Err(RoadmapError::Usage(format!(
            "unknown acceptance operation: {other}"
        ))),
    }
}

pub(super) fn work_command(
    store: &RoadmapStore,
    options: &ParsedOptions,
    json_output: bool,
) -> Result<String, RoadmapError> {
    let item_id = options
        .positionals
        .first()
        .ok_or_else(|| RoadmapError::Usage("Usage: roadmap work <item-id>".into()))?;
    let roadmap = store.load()?;
    let item = find_item(&roadmap, item_id)?.clone();
    if item.status == RoadmapStatus::Dropped || item.status == RoadmapStatus::Passed {
        return Err(RoadmapError::Invalid(format!(
            "cannot work on {} while status is {}",
            item.id, item.status
        )));
    }
    // A dependency the roadmap does not hold is refused by name: skipped,
    // the item would start as if that dependency had passed.
    let mut blockers = Vec::new();
    for dependency in &item.depends_on {
        let dependency = find_item(&roadmap, dependency)?;
        if dependency.status != RoadmapStatus::Passed {
            blockers.push(format!("{} ({})", dependency.id, dependency.status));
        }
    }
    if !blockers.is_empty() {
        return Err(RoadmapError::Invalid(format!(
            "{} is blocked by unresolved dependencies: {}",
            item.id,
            blockers.join(", ")
        )));
    }
    let plan = format!(
        "Roadmap item {}: {}\n\nAcceptance criteria:\n{}",
        item.id,
        item.title,
        item.acceptance
            .iter()
            .map(|criterion| format!("- [{}] {}", criterion.id, criterion.text))
            .collect::<Vec<_>>()
            .join("\n")
    );
    let todos = item
        .acceptance
        .iter()
        .map(|criterion| {
            (
                format!("{}: {}", criterion.id, criterion.text),
                "pending".to_string(),
            )
        })
        .collect::<Vec<_>>();
    crate::slash::activate_roadmap_work(
        &store.cwd,
        &item.id,
        &format!("Complete roadmap item {}: {}", item.id, item.title),
        &plan,
        &todos,
    )?;
    let session_path = crate::agent::record_roadmap_event(
        &store.cwd,
        "roadmap_item_started",
        json!({
            "itemId": item.id,
            "revision": roadmap.revision,
            "artifactPolicy": "new session artifacts and branches inherit activeRoadmapItem"
        }),
    )?;
    if json_output {
        format_json(&json!({
            "itemId": item.id,
            "revision": roadmap.revision,
            "sessionPath": session_path.to_string_lossy(),
            "goal": format!("Complete roadmap item {}: {}", item.id, item.title),
            "todoCount": todos.len(),
        }))
    } else {
        Ok(format!(
            "Roadmap work activated for {}. Goal, plan, and {} todos now point to this item.\nSession: {}\n",
            item.id,
            todos.len(),
            session_path.display()
        ))
    }
}
