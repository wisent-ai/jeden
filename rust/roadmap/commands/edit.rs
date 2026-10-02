//! Changing what an item says, and lifting a block: the counterparts of
//! `add` and `block`. What can be added can be edited; what can be blocked
//! can be unblocked once the thing it waited on is there.

use super::super::model::{RoadmapError, RoadmapStatus};
use super::super::normalize::{find_item, find_item_mut};
use super::super::store::RoadmapStore;
use super::options::{expected_revision, format_json, ParsedOptions};
use serde_json::json;

const EDITABLE: &str = "title area priority summary implementation rationale implementation-order";

/// `roadmap edit <id> --<field> <value>…`: replace the named text fields of
/// one item and nothing else. Status, dependencies, acceptance and evidence
/// have their own commands; a call that names no field is a usage error so
/// a typo in a flag never passes as an edit.
pub(super) fn edit_command(
    store: &RoadmapStore,
    options: &ParsedOptions,
    json_output: bool,
) -> Result<String, RoadmapError> {
    let id = options.positionals.first().ok_or_else(|| {
        RoadmapError::Usage(format!(
            "Usage: roadmap edit <id> [--title <text>] [--area <area>] [--priority <P0|P1|P2|P3>] \
             [--summary <text>] [--implementation <text>] [--rationale <text>] \
             [--implementation-order <text>] [--revision <n>]; editable fields: {EDITABLE}"
        ))
    })?;
    let changes = EDITABLE
        .split(' ')
        .filter_map(|field| options.one(field).map(|value| (field, value.to_string())))
        .collect::<Vec<_>>();
    if changes.is_empty() {
        return Err(RoadmapError::Usage(format!(
            "roadmap edit {id} changes nothing; name at least one field: {EDITABLE}"
        )));
    }
    if let Some((_, title)) = changes.iter().find(|(field, _)| *field == "title") {
        if title.trim().is_empty() {
            return Err(RoadmapError::Usage("roadmap edit: --title cannot be empty".into()));
        }
    }
    let revision = expected_revision(store, options)?;
    let fields = changes
        .iter()
        .map(|(field, _)| *field)
        .collect::<Vec<_>>();
    let roadmap = store.mutate(
        revision,
        "roadmap_item_updated",
        json!({"itemId": id, "operation": "edit", "fields": fields}),
        |roadmap| {
            let item = find_item_mut(roadmap, id)?;
            for (field, value) in &changes {
                let slot = match *field {
                    "title" => &mut item.title,
                    "area" => &mut item.area,
                    "priority" => &mut item.priority,
                    "summary" => &mut item.summary,
                    "implementation" => &mut item.implementation,
                    "rationale" => &mut item.rationale,
                    _ => &mut item.implementation_order,
                };
                *slot = value.clone();
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

/// `roadmap unblock <id> [reason]`: the thing an item waited on is there.
/// The item returns to `planned`, its external prerequisites are cleared and
/// the reason records what arrived. An item that is not blocked is refused
/// with the status it has, so an unblock never hides a wrong id.
pub(super) fn unblock_command(
    store: &RoadmapStore,
    options: &ParsedOptions,
    json_output: bool,
) -> Result<String, RoadmapError> {
    let id = options
        .positionals
        .first()
        .ok_or_else(|| RoadmapError::Usage("Usage: roadmap unblock <id> [reason]".into()))?;
    let positional_reason = options
        .positionals
        .get(1..)
        .unwrap_or_default()
        .join(" ");
    let reason = options.one("reason").map(str::to_string).or_else(|| {
        (!positional_reason.trim().is_empty()).then(|| positional_reason.trim().to_string())
    });
    let revision = expected_revision(store, options)?;
    let roadmap = store.mutate(
        revision,
        "roadmap_item_updated",
        json!({"itemId": id, "operation": "unblock", "status": RoadmapStatus::Planned.as_str(), "reason": reason.clone()}),
        |roadmap| {
            let item = find_item_mut(roadmap, id)?;
            if item.status != RoadmapStatus::ExternalBlocked {
                return Err(RoadmapError::Invalid(format!(
                    "{} is {}, not external_blocked; there is no block to lift",
                    item.id, item.status
                )));
            }
            item.status = RoadmapStatus::Planned;
            item.external_prerequisites.clear();
            item.reason = reason;
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

/// `roadmap depends|undepends <id> <dependency-id>`: attach or detach one
/// dependency edge. Both are the same item mutation; the payload's
/// `operation` is what tells a reader which way it went.
pub(super) fn dependency_command(
    store: &RoadmapStore,
    options: &ParsedOptions,
    command: &str,
    json_output: bool,
) -> Result<String, RoadmapError> {
    let id = options.positionals.first().ok_or_else(|| {
        RoadmapError::Usage(format!("Usage: roadmap {command} <id> <dependency-id>"))
    })?;
    let dependency = options.positionals.get(1).ok_or_else(|| {
        RoadmapError::Usage(format!("Usage: roadmap {command} <id> <dependency-id>"))
    })?;
    let revision = expected_revision(store, options)?;
    let add = command == "depends";
    let roadmap = store.mutate(
        revision,
        "roadmap_item_updated",
        json!({"itemId": id, "dependencyId": dependency, "operation": command}),
        |roadmap| {
            if find_item(roadmap, dependency).is_err() {
                return Err(RoadmapError::NotFound(dependency.clone()));
            }
            let item = find_item_mut(roadmap, id)?;
            if add {
                item.depends_on.push(dependency.to_ascii_uppercase());
            } else {
                let before = item.depends_on.len();
                item.depends_on
                    .retain(|value| !value.eq_ignore_ascii_case(dependency));
                if before == item.depends_on.len() {
                    return Err(RoadmapError::Invalid(format!(
                        "{} does not depend on {}",
                        item.id, dependency
                    )));
                }
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
