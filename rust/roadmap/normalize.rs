//! Putting the roadmap into one canonical shape, and refusing the states that
//! cannot be true.
//!
//! Split out of `roadmap/mod.rs`, which had grown past the module line cap.

use super::model::{RoadmapError, RoadmapFile, RoadmapItem};
use std::collections::{BTreeMap, BTreeSet};

pub(super) fn normalize(roadmap: &mut RoadmapFile) {
    for item in &mut roadmap.items {
        item.id = item.id.trim().to_ascii_uppercase();
        item.title = item.title.trim().to_string();
        item.area = item.area.trim().to_ascii_lowercase();
        item.priority = item.priority.trim().to_ascii_uppercase();
        item.summary = item.summary.trim().to_string();
        item.depends_on.sort();
        item.depends_on.dedup();
        item.capabilities.sort();
        item.capabilities.dedup();
        item.external_prerequisites.sort();
        item.external_prerequisites.dedup();
        item.acceptance
            .sort_by(|left, right| left.id.cmp(&right.id));
        item.evidence.sort_by(|left, right| {
            (&left.acceptance_id, &left.uri).cmp(&(&right.acceptance_id, &right.uri))
        });
    }
    roadmap.items.sort_by(|left, right| left.id.cmp(&right.id));
}

pub(super) fn cycle_errors(roadmap: &RoadmapFile) -> Vec<String> {
    fn visit(
        id: &str,
        dependencies: &BTreeMap<&str, Vec<&str>>,
        visiting: &mut BTreeSet<String>,
        visited: &mut BTreeSet<String>,
        errors: &mut Vec<String>,
    ) {
        if visited.contains(id) {
            return;
        }
        if !visiting.insert(id.to_string()) {
            errors.push(format!("dependency cycle includes {id}"));
            return;
        }
        if let Some(next) = dependencies.get(id) {
            for dependency in next {
                visit(dependency, dependencies, visiting, visited, errors);
            }
        }
        visiting.remove(id);
        visited.insert(id.to_string());
    }

    let dependencies = roadmap
        .items
        .iter()
        .map(|item| {
            (
                item.id.as_str(),
                item.depends_on
                    .iter()
                    .map(String::as_str)
                    .collect::<Vec<_>>(),
            )
        })
        .collect::<BTreeMap<_, _>>();
    let mut visiting = BTreeSet::new();
    let mut visited = BTreeSet::new();
    let mut errors = Vec::new();
    for item in &roadmap.items {
        visit(
            &item.id,
            &dependencies,
            &mut visiting,
            &mut visited,
            &mut errors,
        );
    }
    errors
}

/// The first number a roadmap gives an id it numbers itself, and the step
/// from the highest number to the next.
pub(super) const FIRST_NUMBER: u64 = crate::tally::FIRST;

/// The number after the highest `<prefix><digits>` among `ids`, or the first
/// when none is numbered so; an id named otherwise is not counted. A number
/// too large to count past is refused, naming its id.
pub(super) fn next_number<'a>(
    ids: impl Iterator<Item = &'a str>,
    prefix: &str,
) -> Result<u64, RoadmapError> {
    let mut highest = None;
    for digits in ids.filter_map(|id| id.strip_prefix(prefix)) {
        if digits.is_empty() || !digits.bytes().all(|byte| byte.is_ascii_digit()) {
            continue;
        }
        let number = digits.parse::<u64>().map_err(|error| {
            RoadmapError::Invalid(format!("{prefix}{digits} cannot be counted: {error}"))
        })?;
        highest = highest.max(Some(number));
    }
    match highest {
        None => Ok(FIRST_NUMBER),
        Some(number) => number.checked_add(FIRST_NUMBER).ok_or_else(|| {
            RoadmapError::Invalid(format!("{prefix}{number} cannot be counted past"))
        }),
    }
}

/// The id the next item is given.
pub(super) fn next_item_id(roadmap: &RoadmapFile) -> Result<String, RoadmapError> {
    let next = next_number(roadmap.items.iter().map(|item| item.id.as_str()), "JED-")?;
    Ok(format!("JED-{next:03}"))
}

/// Who records a roadmap change: USER, or USERNAME where USER is unset. A
/// change nobody is named for is refused, never attributed to "unknown".
pub(super) fn actor() -> Result<String, RoadmapError> {
    let named = match std::env::var("USER") {
        Err(std::env::VarError::NotPresent) => std::env::var("USERNAME"),
        read => read,
    };
    named.map_err(|error| {
        RoadmapError::Invalid(format!(
            "USER or USERNAME must name who records this roadmap change: {error}"
        ))
    })
}

/// When a roadmap change is recorded; a clock that cannot say refuses the
/// change rather than dating it at the epoch.
pub(super) fn now() -> Result<String, RoadmapError> {
    crate::agent::now_stamp()
        .map_err(|error| RoadmapError::Io(format!("the roadmap change cannot be dated: {error}")))
}

pub(crate) fn find_item<'a>(
    roadmap: &'a RoadmapFile,
    id: &str,
) -> Result<&'a RoadmapItem, RoadmapError> {
    roadmap
        .items
        .iter()
        .find(|item| item.id.eq_ignore_ascii_case(id))
        .ok_or_else(|| RoadmapError::NotFound(id.to_string()))
}

pub(crate) fn find_item_mut<'a>(
    roadmap: &'a mut RoadmapFile,
    id: &str,
) -> Result<&'a mut RoadmapItem, RoadmapError> {
    roadmap
        .items
        .iter_mut()
        .find(|item| item.id.eq_ignore_ascii_case(id))
        .ok_or_else(|| RoadmapError::NotFound(id.to_string()))
}
