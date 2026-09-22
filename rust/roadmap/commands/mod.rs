//! Every roadmap command an operator can run.
//!
//! Split out of `roadmap/mod.rs`, which had grown past the module line cap.

use super::model::{AcceptanceCriterion, RoadmapError, RoadmapFile, RoadmapItem, RoadmapStatus};
use super::normalize::{actor, find_item, find_item_mut, next_item_id, now};
use super::{render::render_markdown, store::RoadmapStore};
use serde_json::json;
use std::{path::Path, str::FromStr};

mod acceptance;
mod options;
mod status;

use acceptance::{acceptance_command, work_command};
use options::{expected_revision, format_json, ParsedOptions};
use status::{mutate_status, mutate_status_explicit};

pub fn execute(cwd: &Path, args: &[String], json_output: bool) -> Result<String, RoadmapError> {
    let store = RoadmapStore::new(cwd);
    let command = args.first().map(String::as_str).unwrap_or("list");
    let options = ParsedOptions::parse(args.get(1..).unwrap_or_default())?;
    match command {
        "list" => {
            let roadmap = store.load()?;
            let status = options
                .one("status")
                .map(RoadmapStatus::from_str)
                .transpose()?;
            let area = options.one("area");
            let priority = options.one("priority");
            let items = roadmap
                .items
                .iter()
                .filter(|item| status.as_ref().map(|v| v == &item.status).unwrap_or(true))
                .filter(|item| area.map(|v| v == item.area).unwrap_or(true))
                .filter(|item| priority.map(|v| v == item.priority).unwrap_or(true))
                .cloned()
                .collect::<Vec<_>>();
            if json_output {
                format_json(&json!({
                    "schemaVersion": roadmap.schema_version,
                    "revision": roadmap.revision,
                    "items": items,
                }))
            } else if items.is_empty() {
                Ok(format!("No roadmap items matched. (revision {})\n", roadmap.revision))
            } else {
                let mut output = format!("Roadmap revision {}\n", roadmap.revision);
                for item in items {
                    output.push_str(&format!(
                        "{}\t{}\t{}\t{}\t{}\n",
                        item.id, item.status, item.priority, item.area, item.title
                    ));
                }
                Ok(output)
            }
        }
        "show" => {
            let id = options
                .positionals
                .first()
                .ok_or_else(|| RoadmapError::Usage("Usage: roadmap show <id>".into()))?;
            let roadmap = store.load()?;
            let item = find_item(&roadmap, id)?;
            if json_output {
                format_json(item)
            } else {
                Ok(render_markdown(&RoadmapFile {
                    schema_version: roadmap.schema_version,
                    revision: roadmap.revision,
                    context: String::new(),
                    items: vec![item.clone()],
                }))
            }
        }
        "graph" => {
            let graph = store.graph()?;
            if json_output {
                format_json(&graph)
            } else {
                let mut output = format!("Roadmap dependency graph (revision {})\n", graph.revision);
                for node in &graph.nodes {
                    output.push_str(&format!("{} [{}] {}\n", node.id, node.status, node.title));
                }
                for edge in &graph.edges {
                    output.push_str(&format!("{} -> {}\n", edge.from, edge.to));
                }
                Ok(output)
            }
        }
        "add" => {
            let positional_title = options.positionals.join(" ");
            if options.one("title").is_some() && !positional_title.trim().is_empty() {
                return Err(RoadmapError::Usage(
                    "roadmap add accepts the title either positionally or through --title, not both"
                        .into(),
                ));
            }
            let title = options
                .one("title")
                .map(str::to_string)
                .unwrap_or(positional_title);
            if title.trim().is_empty() {
                return Err(RoadmapError::Usage(
                    "Usage: roadmap add <title> | --title <title> --area <area> --priority <P0|P1|P2|P3> --summary <text> --acceptance <text> [--revision <n>]".into(),
                ));
            }
            let revision = expected_revision(&store, &options)?;
            let area = options.one("area").unwrap_or("general").to_string();
            let priority = options.one("priority").unwrap_or("P2").to_string();
            let summary = options.one("summary").unwrap_or(&title).to_string();
            let acceptance = options
                .many("acceptance")
                .into_iter()
                .enumerate()
                .map(|(index, text)| AcceptanceCriterion {
                    id: format!("acceptance-{}", index + 1),
                    text,
                })
                .collect::<Vec<_>>();
            let dependencies = options
                .many("depends-on")
                .into_iter()
                .flat_map(|value| value.split(',').map(str::to_string).collect::<Vec<_>>())
                .collect::<Vec<_>>();
            let capabilities = options.many("capability");
            let prerequisites = options.many("external-prerequisite");
            let status = options
                .one("status")
                .map(RoadmapStatus::from_str)
                .transpose()?
                .unwrap_or(RoadmapStatus::Backlog);
            let created_at = now();
            let created_by = actor();
            let mut created_id = String::new();
            let roadmap = store.mutate(
                revision,
                "roadmap_item_created",
                json!({"title": title}),
                |roadmap| {
                    let id = options
                        .one("id")
                        .map(str::to_ascii_uppercase)
                        .unwrap_or_else(|| next_item_id(roadmap));
                    created_id = id.clone();
                    roadmap.items.push(RoadmapItem {
                        id,
                        title: title.clone(),
                        area,
                        priority,
                        status,
                        summary,
                        implementation: options
                            .one("implementation")
                            .unwrap_or_default()
                            .to_string(),
                        rationale: options
                            .one("rationale")
                            .unwrap_or_default()
                            .to_string(),
                        implementation_order: options
                            .one("implementation-order")
                            .unwrap_or_default()
                            .to_string(),
                        acceptance,
                        depends_on: dependencies,
                        capabilities,
                        external_prerequisites: prerequisites,
                        evidence: Vec::new(),
                        reason: None,
                        created_at,
                        created_by,
                    });
                    Ok(())
                },
            )?;
            if json_output {
                format_json(find_item(&roadmap, &created_id)?)
            } else {
                Ok(format!(
                    "Created {} at roadmap revision {}.\n",
                    created_id, roadmap.revision
                ))
            }
        }
        "drop" => mutate_status(
            &store,
            &options,
            RoadmapStatus::Dropped,
            true,
            json_output,
        ),
        "start" => mutate_status(
            &store,
            &options,
            RoadmapStatus::InProgress,
            false,
            json_output,
        ),
        "implemented" => mutate_status(
            &store,
            &options,
            RoadmapStatus::Implemented,
            false,
            json_output,
        ),
        "block" => mutate_status(
            &store,
            &options,
            RoadmapStatus::ExternalBlocked,
            false,
            json_output,
        ),
        "pass" => mutate_status(
            &store,
            &options,
            RoadmapStatus::Passed,
            false,
            json_output,
        ),
        "status" => {
            let id = options
                .positionals
                .first()
                .ok_or_else(|| RoadmapError::Usage("Usage: roadmap status <id> <status>".into()))?;
            let status = options
                .positionals
                .get(1)
                .ok_or_else(|| RoadmapError::Usage("Usage: roadmap status <id> <status>".into()))?
                .parse()?;
            mutate_status_explicit(&store, &options, id, status, false, 2, json_output)
        }
        "depends" | "undepends" => {
            let id = options.positionals.first().ok_or_else(|| {
                RoadmapError::Usage(format!("Usage: roadmap {command} <id> <dependency-id>"))
            })?;
            let dependency = options.positionals.get(1).ok_or_else(|| {
                RoadmapError::Usage(format!("Usage: roadmap {command} <id> <dependency-id>"))
            })?;
            let revision = expected_revision(&store, &options)?;
            let add = command == "depends";
            // Attach and detach are the same item mutation; the payload's
            // `operation` is what tells a reader which way it went.
            let event = "roadmap_item_updated";
            let roadmap = store.mutate(
                revision,
                event,
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
        "acceptance" => acceptance_command(&store, &options, json_output),
        "check" => {
            let report = store.check();
            if json_output {
                format_json(&report)
            } else if report.ok {
                Ok(format!(
                    "Roadmap valid: schema {}, revision {}, {} items.\n",
                    report.schema_version, report.revision, report.item_count
                ))
            } else {
                Err(RoadmapError::Invalid(report.errors.join("\n")))
            }
        }
        "work" => work_command(&store, &options, json_output),
        other => Err(RoadmapError::Usage(format!(
            "unknown roadmap command: {other}; expected list|show|add|drop|start|implemented|block|pass|status|depends|undepends|graph|acceptance|check|work"
        ))),
    }
}
