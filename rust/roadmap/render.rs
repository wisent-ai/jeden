//! Writing the roadmap out as the document a person reads, generated from the
//! file rather than maintained by hand.
//!
//! Split out of `roadmap/mod.rs`, which had grown past the module line cap.

use super::model::RoadmapFile;

pub(super) fn render_markdown(roadmap: &RoadmapFile) -> String {
    let mut out = String::new();
    out.push_str("# Jeden Production Roadmap\n\n");
    out.push_str(&format!(
        "Schema version: `{}` · Revision: `{}` · Items: `{}`\n\n",
        roadmap.schema_version,
        roadmap.revision,
        roadmap.items.len()
    ));
    out.push_str("## Status legend\n\n");
    out.push_str("`backlog` · `planned` · `in_progress` · `implemented` · `not_run` · `failed` · `external_blocked` · `passed` · `dropped`\n\n");
    if !roadmap.context.trim().is_empty() {
        out.push_str("## Migrated production-program context\n\n");
        out.push_str("The preserved context below describes the original 23-scope certification program. The canonical item list that follows is versioned independently and may extend that original program.\n\n");
        out.push_str(roadmap.context.trim());
        out.push_str("\n\n");
    }
    for item in &roadmap.items {
        out.push_str(&format!("## {}. {}\n\n", item.id, item.title));
        out.push_str(&format!(
            "- **Area:** `{}`\n- **Priority:** `{}`\n- **Status:** `{}`\n",
            item.area, item.priority, item.status
        ));
        out.push_str(&format!("- **Summary:** {}\n", item.summary));
        if !item.depends_on.is_empty() {
            out.push_str(&format!(
                "- **Depends on:** {}\n",
                item.depends_on.join(", ")
            ));
        }
        if !item.capabilities.is_empty() {
            out.push_str(&format!(
                "- **Capabilities:** {}\n",
                item.capabilities.join(", ")
            ));
        }
        if !item.external_prerequisites.is_empty() {
            out.push_str("- **External prerequisites:**\n");
            for prerequisite in &item.external_prerequisites {
                out.push_str(&format!("  - {}\n", prerequisite));
            }
        }
        if let Some(reason) = &item.reason {
            out.push_str(&format!("- **Status reason:** {}\n", reason));
        }
        if !item.implementation.is_empty() {
            out.push_str(&format!(
                "\n### Files and modules\n\n{}\n",
                item.implementation
            ));
        }
        if !item.rationale.is_empty() {
            out.push_str(&format!("\n### Rationale\n\n{}\n", item.rationale));
        }
        if !item.implementation_order.is_empty() {
            out.push_str(&format!(
                "\n### Implementation order\n\n{}\n",
                item.implementation_order
            ));
        }
        out.push_str("\n### Acceptance criteria\n\n");
        for criterion in &item.acceptance {
            let has_evidence = item
                .evidence
                .iter()
                .any(|evidence| evidence.acceptance_id.as_deref() == Some(&criterion.id));
            out.push_str(&format!(
                "- [{}] **{}** — {}\n",
                if has_evidence { "x" } else { " " },
                criterion.id,
                criterion.text
            ));
        }
        if !item.evidence.is_empty() {
            out.push_str("\n### Evidence\n\n");
            for evidence in &item.evidence {
                let criterion = evidence
                    .acceptance_id
                    .as_deref()
                    .map(|id| format!(" ({id})"))
                    .unwrap_or_default();
                out.push_str(&format!("- `{}`{}\n", evidence.uri, criterion));
            }
        }
        out.push('\n');
    }
    while out.ends_with("\n\n") {
        out.pop();
    }
    out
}
