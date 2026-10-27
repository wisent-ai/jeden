//! Every roadmap command an operator can run.
//!
//! Split out of `roadmap/mod.rs`, which had grown past the module line cap.

use super::model::{AcceptanceCriterion, RoadmapError, RoadmapFile, RoadmapItem, RoadmapStatus};
use super::normalize::{actor, find_item, next_item_id, now, FIRST_NUMBER};
use super::{render::render_markdown, store::RoadmapStore};
use serde_json::json;
use std::{path::Path, str::FromStr};

mod acceptance;
mod edit;
mod options;
mod status;

use acceptance::{acceptance_command, work_command};
use edit::{dependency_command, edit_command, unblock_command, DependencyChange};
use options::{expected_revision, format_json, ParsedOptions};
use status::{mutate_status, mutate_status_explicit};

pub fn execute(cwd: &Path, args: &[String], json_output: bool) -> Result<String, RoadmapError> {
    let store = RoadmapStore::new(cwd);
    // `jeden roadmap` with no command lists the roadmap.
    let (command, rest) = match args {
        [] => ("list", args),
        [command, rest @ ..] => (command.as_str(), rest),
    };
    let options = ParsedOptions::parse(rest)?;
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
                .filter(|item| status.as_ref().is_none_or(|wanted| *wanted == item.status))
                .filter(|item| area.is_none_or(|wanted| wanted == item.area))
                .filter(|item| priority.is_none_or(|wanted| wanted == item.priority))
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
            let title = match options.one("title") {
                Some(title) => title.to_string(),
                None => positional_title,
            };
            if title.trim().is_empty() {
                return Err(RoadmapError::Usage(
                    "Usage: roadmap add <title> | --title <title> --area <area> --priority <P0|P1|P2|P3> --summary <text> --acceptance <text> [--revision <n>]".into(),
                ));
            }
            let required = |name: &str| {
                options.one(name).map(str::to_string).ok_or_else(|| {
                    RoadmapError::Usage(format!(
                        "roadmap add requires --{name}: an item is filed under the area, priority and summary its author gives it"
                    ))
                })
            };
            let area = required("area")?;
            let priority = required("priority")?;
            let summary = required("summary")?;
            let revision = expected_revision(&store, &options)?;
            // An optional text the author did not give is written as empty.
            let optional = |name: &str| match options.one(name) {
                Some(text) => text.to_string(),
                None => String::new(),
            };
            let acceptance = (FIRST_NUMBER..)
                .zip(options.many("acceptance"))
                .map(|(number, text)| AcceptanceCriterion {
                    id: format!("acceptance-{number}"),
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
            // A new item starts in the backlog unless its author names a status.
            let status = match options.one("status") {
                Some(named) => RoadmapStatus::from_str(named)?,
                None => RoadmapStatus::Backlog,
            };
            let created_at = now()?;
            let created_by = actor()?;
            let mut created_id = String::new();
            let roadmap = store.mutate(
                revision,
                "roadmap_item_created",
                json!({"title": title}),
                |roadmap| {
                    let id = match options.one("id") {
                        Some(named) => named.to_ascii_uppercase(),
                        None => next_item_id(roadmap)?,
                    };
                    created_id = id.clone();
                    roadmap.items.push(RoadmapItem {
                        id,
                        title: title.clone(),
                        area,
                        priority,
                        status,
                        summary,
                        implementation: optional("implementation"),
                        rationale: optional("rationale"),
                        implementation_order: optional("implementation-order"),
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
        "edit" => edit_command(&store, &options, json_output),
        "unblock" => unblock_command(&store, &options, json_output),
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
            let [id, status, reason_words @ ..] = options.positionals.as_slice() else {
                return Err(RoadmapError::Usage(
                    "Usage: roadmap status <id> <status>".into(),
                ));
            };
            let status = status.parse()?;
            mutate_status_explicit(&store, &options, id, status, false, reason_words, json_output)
        }
        "depends" => dependency_command(&store, &options, DependencyChange::Depends, json_output),
        "undepends" => {
            dependency_command(&store, &options, DependencyChange::Undepends, json_output)
        }
        "acceptance" => acceptance_command(&store, &options, json_output),
        "check" => {
            let report = store.check();
            if json_output {
                format_json(&report)
            } else if let (true, Some(schema), Some(revision), Some(items)) = (
                report.ok,
                report.schema_version,
                report.revision,
                report.item_count,
            ) {
                Ok(format!(
                    "Roadmap valid: schema {schema}, revision {revision}, {items} items.\n"
                ))
            } else {
                Err(RoadmapError::Invalid(report.errors.join("\n")))
            }
        }
        "work" => work_command(&store, &options, json_output),
        other => Err(RoadmapError::Usage(format!(
            "unknown roadmap command: {other}; expected list|show|add|edit|drop|start|implemented|block|unblock|pass|status|depends|undepends|graph|acceptance|check|work"
        ))),
    }
}
