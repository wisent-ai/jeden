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

pub(super) fn next_item_id(roadmap: &RoadmapFile) -> String {
    let next = roadmap
        .items
        .iter()
        .filter_map(|item| item.id.strip_prefix("JED-")?.parse::<u64>().ok())
        .max()
        .unwrap_or(0)
        .saturating_add(1);
    format!("JED-{next:03}")
}

pub(super) fn actor() -> String {
    std::env::var("USER")
        .or_else(|_| std::env::var("USERNAME"))
        .unwrap_or_else(|_| "unknown".into())
}

pub(super) fn now() -> String {
    crate::agent::now_stamp()
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
