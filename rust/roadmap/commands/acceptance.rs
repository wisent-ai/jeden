//! The acceptance criteria of a roadmap item, and the evidence attached to
//! them.
//!
//! Split out of `roadmap/mod.rs`, which had grown past the module line cap.

use super::super::model::{AcceptanceCriterion, EvidenceLink, RoadmapError, RoadmapStatus};
use super::super::normalize::{actor, find_item, find_item_mut, now};
use super::super::store::RoadmapStore;
use super::options::{expected_revision, format_json, ParsedOptions};
use serde_json::json;

pub(super) fn acceptance_command(
    store: &RoadmapStore,
    options: &ParsedOptions,
    json_output: bool,
) -> Result<String, RoadmapError> {
    let operation = options
        .positionals
        .first()
        .map(String::as_str)
        .unwrap_or("list");
    let item_id = options.positionals.get(1).ok_or_else(|| {
        RoadmapError::Usage("Usage: roadmap acceptance <list|add|evidence> <item-id> ...".into())
    })?;
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
    match operation {
        "add" => {
            let text = options.positionals.get(2..).unwrap_or_default().join(" ");
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
                    criterion_id = options
                        .one("id")
                        .map(str::to_string)
                        .unwrap_or_else(|| format!("acceptance-{}", item.acceptance.len() + 1));
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
            let criterion_id = options.positionals.get(2).ok_or_else(|| {
                RoadmapError::Usage(
                    "Usage: roadmap acceptance evidence <item-id> <acceptance-id> <artifact-uri>"
                        .into(),
                )
            })?;
            let uri = options.positionals.get(3).ok_or_else(|| {
                RoadmapError::Usage(
                    "Usage: roadmap acceptance evidence <item-id> <acceptance-id> <artifact-uri>"
                        .into(),
                )
            })?;
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
                        added_at: now(),
                        added_by: actor(),
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
    let blockers = item
        .depends_on
        .iter()
        .filter_map(|dependency| find_item(&roadmap, dependency).ok())
        .filter(|dependency| dependency.status != RoadmapStatus::Passed)
        .map(|dependency| format!("{} ({})", dependency.id, dependency.status))
        .collect::<Vec<_>>();
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
