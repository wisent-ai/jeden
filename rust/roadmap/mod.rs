//! The production roadmap: what is planned, what is in progress, and what
//! proves an item is done.

use crate::capability::{CapabilityDescriptor, CapabilityHealth, CapabilityKind, FunctionTarget};
use crate::tui::{PickerItem, PickerSpec};
use serde::Serialize;
use std::path::Path;

mod commands;
mod model;
mod normalize;
mod render;
mod store;

pub use commands::execute;
pub use model::{
    AcceptanceCriterion, EvidenceLink, RoadmapError, RoadmapFile, RoadmapItem, RoadmapStatus,
};
pub use store::RoadmapStore;

pub const ROADMAP_SCHEMA_VERSION: u32 = 1;

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RoadmapGraph {
    pub revision: u64,
    pub nodes: Vec<RoadmapGraphNode>,
    pub edges: Vec<RoadmapGraphEdge>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RoadmapGraphNode {
    pub id: String,
    pub title: String,
    pub status: RoadmapStatus,
    pub priority: String,
    pub area: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RoadmapGraphEdge {
    pub from: String,
    pub to: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CheckReport {
    pub ok: bool,
    pub schema_version: u32,
    pub revision: u64,
    pub item_count: usize,
    pub errors: Vec<String>,
}

pub fn split_command_line(input: &str) -> Result<Vec<String>, RoadmapError> {
    let mut args = Vec::new();
    let mut current = String::new();
    let mut quote = None;
    let mut escaped = false;
    for character in input.chars() {
        if escaped {
            current.push(character);
            escaped = false;
            continue;
        }
        if character == '\\' {
            escaped = true;
            continue;
        }
        if let Some(active) = quote {
            if character == active {
                quote = None;
            } else {
                current.push(character);
            }
            continue;
        }
        if character == '\'' || character == '"' {
            quote = Some(character);
        } else if character.is_whitespace() {
            if !current.is_empty() {
                args.push(std::mem::take(&mut current));
            }
        } else {
            current.push(character);
        }
    }
    if escaped || quote.is_some() {
        return Err(RoadmapError::Usage(
            "unterminated escape or quote in roadmap command".into(),
        ));
    }
    if !current.is_empty() {
        args.push(current);
    }
    Ok(args)
}

pub fn picker(cwd: &Path) -> Result<PickerSpec, RoadmapError> {
    let roadmap = RoadmapStore::new(cwd).load()?;
    let mut items = vec![PickerItem::action(
        "Add roadmap item",
        "/roadmap add --title \"\" --area general --priority P2 --summary \"\" --acceptance \"\"",
    )
    .detail("Prefill the required title, area, priority, summary, and acceptance fields")
    .badge("ADD")
    .prefill()];
    items.extend(roadmap.items.into_iter().map(|item| {
        PickerItem::action(
            format!("{}  {}", item.id, item.title),
            format!("/roadmap show {}", item.id),
        )
        .detail(format!(
            "{} · {} · {}",
            item.status, item.priority, item.area
        ))
        .badge(item.priority)
    }));
    let mut picker = PickerSpec::new("Roadmap", items);
    picker.prompt = "Search roadmap items:".into();
    picker.empty_message = "No roadmap items match".into();
    Ok(picker)
}

pub fn capability_descriptors(cwd: &Path) -> Vec<CapabilityDescriptor> {
    let store = RoadmapStore::new(cwd);
    let path = store.path().to_path_buf();
    let health = match store.load() {
        Ok(roadmap) => {
            let errors = store.validation_errors(&roadmap, false);
            if errors.is_empty() {
                CapabilityHealth::healthy()
            } else {
                CapabilityHealth::unavailable(errors.join("; "))
            }
        }
        Err(error) => CapabilityHealth::unavailable(format!("{}: {error}", path.display())),
    };
    let planned =
        CapabilityHealth::unavailable("Planned by JED-024; no review runtime is executable yet");
    vec![
        CapabilityDescriptor::new(
            "service/roadmap-registry",
            CapabilityKind::Service,
            "jeden-core",
            "Roadmap registry",
            "Versioned repository roadmap with revision-guarded atomic mutations",
            FunctionTarget::Service {
                name: "roadmap-registry".into(),
            },
        )
        .operation("read")
        .operation("mutate")
        .operation("validate")
        .operation("render")
        .health(health),
        CapabilityDescriptor::new(
            "service/review-runtime",
            CapabilityKind::Service,
            "jeden-roadmap",
            "Review runtime",
            "Planned typed review execution contract",
            FunctionTarget::Service {
                name: "review-runtime".into(),
            },
        )
        .operation("planned")
        .health(planned.clone()),
        CapabilityDescriptor::new(
            "slash/review",
            CapabilityKind::SlashCommand,
            "jeden-roadmap",
            "/review",
            "Planned native review command",
            FunctionTarget::BuiltinSlash {
                command: "review".into(),
            },
        )
        .operation("planned")
        .health(planned.clone()),
        CapabilityDescriptor::new(
            "view/review",
            CapabilityKind::View,
            "jeden-roadmap",
            "Review",
            "Planned native review picker",
            FunctionTarget::NativeView {
                command: "review".into(),
            },
        )
        .operation("planned")
        .health(planned),
    ]
}
