use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use crate::cli::config::{AlwaysApplyRuleConfig, ContextConfig, RulesConfig};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ContextKind {
    Rule,
    Context,
}

#[derive(Debug, Clone)]
pub(crate) struct Provenance {
    pub(crate) path: PathBuf,
    pub(crate) imported_by: Option<PathBuf>,
}

#[derive(Debug, Clone)]
pub(crate) struct ContextEntry {
    pub(crate) id: String,
    pub(crate) kind: ContextKind,
    pub(crate) always_apply: bool,
    pub(crate) precedence: usize,
    pub(crate) content: String,
    pub(crate) provenance: Vec<Provenance>,
}

#[derive(Debug, Clone, Default)]
pub(crate) struct ContextBundle {
    pub(crate) entries: Vec<ContextEntry>,
    pub(crate) warnings: Vec<String>,
}

impl ContextBundle {
    pub(crate) fn render_for_prompt(&self) -> String {
        if self.entries.is_empty() && self.warnings.is_empty() {
            return String::new();
        }
        let mut sections = Vec::new();
        if !self.warnings.is_empty() {
            sections.push(format!(
                "Context discovery warnings (must be surfaced, not ignored):\n- {}",
                self.warnings.join("\n- ")
            ));
        }
        for entry in &self.entries {
            if entry.always_apply {
                continue;
            }
            let kind = match entry.kind {
                ContextKind::Rule => "rule",
                ContextKind::Context => "context",
            };
            let sources = entry
                .provenance
                .iter()
                .map(|source| match &source.imported_by {
                    Some(parent) => format!(
                        "{} (imported by {})",
                        source.path.display(),
                        parent.display()
                    ),
                    None => source.path.display().to_string(),
                })
                .collect::<Vec<_>>()
                .join(" -> ");
            sections.push(format!(
                "[{}; kind={}; precedence={}; always-apply={}; provenance={}]\n{}",
                entry.id, kind, entry.precedence, entry.always_apply, sources, entry.content
            ));
        }
        if sections.is_empty() {
            return String::new();
        }
        format!("\n\nProject context:\n{}", sections.join("\n\n"))
    }
}

mod budget;
mod sources;

use budget::Budget;
use sources::{ancestor_chain, load_expanded, DESCRIPTORS};

pub(crate) fn project_root(cwd: &Path) -> Result<PathBuf, String> {
    let canonical = cwd
        .canonicalize()
        .map_err(|error| format!("cannot resolve cwd {}: {error}", cwd.display()))?;
    let mut context_root = None;
    // A marker that cannot be examined is refused naming it: read as absent,
    // a repository root was passed over and a farther directory, or the cwd
    // itself, became the project root the context was read from.
    let present = crate::tool_runtime::runtime_ops::platform::presence::present;
    for ancestor in canonical.ancestors() {
        if present(&ancestor.join(".git"))? {
            return Ok(ancestor.to_path_buf());
        }
        for descriptor in DESCRIPTORS.iter() {
            let file = ancestor.join(descriptor.relative);
            if present(&file)? && file.is_file() {
                context_root = Some(ancestor.to_path_buf());
                break;
            }
        }
    }
    // With neither a repository nor a context file above it, the directory is
    // its own project root.
    match context_root {
        Some(root) => Ok(root),
        None => Ok(canonical),
    }
}

pub(crate) fn discover_context(
    cwd: &Path,
    config: &ContextConfig,
    rules: &RulesConfig,
) -> Result<ContextBundle, String> {
    let canonical_cwd = cwd
        .canonicalize()
        .map_err(|error| format!("cannot resolve cwd {}: {error}", cwd.display()))?;
    let root = project_root(&canonical_cwd)?;
    let mut budget = Budget::new(config.max_bytes);
    // Explicit sticky rules are load-bearing policy. Reserve their budget first,
    // while assigning their final precedence after discovered project files.
    let mut configured_entries = Vec::new();
    append_configured_rules(
        &root,
        &mut budget,
        &mut configured_entries,
        &rules.always_apply,
    )?;
    // An entry's precedence is its position: later entries apply over earlier.
    let mut entries = Vec::new();
    for directory in ancestor_chain(&root, &canonical_cwd) {
        for descriptor in DESCRIPTORS {
            let path = directory.join(descriptor.relative);
            if !path.is_file() {
                continue;
            }
            let mut provenance = Vec::new();
            let content = load_expanded(
                &path,
                &root,
                &mut budget,
                &mut Vec::new(),
                &mut provenance,
                None,
            )?;
            entries.push(ContextEntry {
                id: descriptor.relative.to_string(),
                kind: descriptor.kind,
                always_apply: descriptor.kind == ContextKind::Rule,
                precedence: entries.len(),
                content,
                provenance,
            });
        }
    }
    for mut entry in configured_entries {
        entry.precedence = entries.len();
        entries.push(entry);
    }
    let mut seen_ids = BTreeSet::new();
    for entry in &entries {
        if !seen_ids.insert(entry.id.clone()) {
            budget.warnings.push(format!(
                "rule id '{}' is repeated; higher precedence applies later",
                entry.id
            ));
        }
    }
    Ok(ContextBundle {
        entries,
        warnings: budget.warnings,
    })
}

fn append_configured_rules(
    jail: &Path,
    budget: &mut Budget,
    entries: &mut Vec<ContextEntry>,
    rules: &[AlwaysApplyRuleConfig],
) -> Result<(), String> {
    for rule in rules {
        let (content, provenance) = match (&rule.content, &rule.source) {
            (Some(content), None) => (
                budget.include(Path::new("<config>"), content),
                vec![Provenance {
                    path: PathBuf::from("<config>"),
                    imported_by: None,
                }],
            ),
            (None, Some(source)) => {
                let mut provenance = Vec::new();
                let content = load_expanded(
                    &jail.join(source),
                    jail,
                    budget,
                    &mut Vec::new(),
                    &mut provenance,
                    None,
                )?;
                (content, provenance)
            }
            _ => {
                return Err(format!(
                    "always-apply rule '{}' must set exactly one of content or source",
                    rule.id
                ))
            }
        };
        entries.push(ContextEntry {
            id: rule.id.clone(),
            kind: ContextKind::Rule,
            always_apply: true,
            precedence: entries.len(),
            content,
            provenance,
        });
    }
    Ok(())
}
