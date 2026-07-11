use std::collections::BTreeSet;
use std::fs;
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
    pub(crate) root: PathBuf,
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

#[derive(Clone, Copy)]
struct Descriptor {
    relative: &'static str,
    kind: ContextKind,
}

// Ordered from lowest to highest precedence within one directory. The array is
// the stable schema descriptor; discovery never depends on directory iteration.
const DESCRIPTORS: &[Descriptor] = &[
    Descriptor { relative: "CLAUDE.md", kind: ContextKind::Rule },
    Descriptor { relative: "AGENTS.md", kind: ContextKind::Rule },
    Descriptor { relative: "RULES.md", kind: ContextKind::Rule },
    Descriptor { relative: "JEDEN.md", kind: ContextKind::Rule },
    Descriptor { relative: ".jeden/instructions.md", kind: ContextKind::Rule },
    Descriptor { relative: ".jeden/context.md", kind: ContextKind::Context },
];

struct Budget {
    max_bytes: usize,
    max_tokens: usize,
    used_bytes: usize,
    used_tokens: usize,
    warned_paths: BTreeSet<PathBuf>,
    files_read: usize,
    warnings: Vec<String>,
}

impl Budget {
    fn include(&mut self, path: &Path, text: &str) -> String {
        let remaining_bytes = self.max_bytes.saturating_sub(self.used_bytes);
        let remaining_chars = self
            .max_tokens
            .saturating_sub(self.used_tokens)
            .saturating_mul(4);
        let byte_limit = remaining_bytes.min(remaining_chars);
        let mut end = byte_limit.min(text.len());
        while end > 0 && !text.is_char_boundary(end) {
            end -= 1;
        }
        let included = &text[..end];
        self.used_bytes = self.used_bytes.saturating_add(included.len());
        self.used_tokens = self
            .used_tokens
            .saturating_add((included.chars().count().saturating_add(3)) / 4);
        if end < text.len() && self.warned_paths.insert(path.to_path_buf()) {
            self.warnings.push(format!(
                "budget exceeded while reading {}: included {} of {} bytes (limits: {} bytes, ~{} tokens)",
                path.display(),
                end,
                text.len(),
                self.max_bytes,
                self.max_tokens
            ));
        }
        included.to_string()
    }
}

pub(crate) fn project_root(cwd: &Path) -> Result<PathBuf, String> {
    let canonical = cwd
        .canonicalize()
        .map_err(|error| format!("cannot resolve cwd {}: {error}", cwd.display()))?;
    let mut context_root = None;
    for ancestor in canonical.ancestors() {
        if ancestor.join(".git").exists() {
            return Ok(ancestor.to_path_buf());
        }
        if DESCRIPTORS
            .iter()
            .any(|descriptor| ancestor.join(descriptor.relative).is_file())
        {
            context_root = Some(ancestor.to_path_buf());
        }
    }
    Ok(context_root.unwrap_or(canonical))
}

fn parse_import(line: &str) -> Option<&str> {
    let trimmed = line.trim();
    let raw = trimmed
        .strip_prefix("@import ")
        .or_else(|| trimmed.strip_prefix("@include "))
        .or_else(|| trimmed.strip_prefix('@').filter(|rest| !rest.contains(char::is_whitespace)))?;
    let path = raw.trim().trim_matches(['\'', '"']);
    (!path.is_empty()).then_some(path)
}

fn load_expanded(
    path: &Path,
    jail: &Path,
    budget: &mut Budget,
    stack: &mut Vec<PathBuf>,
    provenance: &mut Vec<Provenance>,
    imported_by: Option<&Path>,
) -> Result<String, String> {
    let canonical = path
        .canonicalize()
        .map_err(|error| format!("context import {} cannot be resolved: {error}", path.display()))?;
    budget.files_read = budget.files_read.saturating_add(1);
    if budget.files_read > 256 {
        return Err(format!(
            "context import limit exceeded at {} (maximum 256 files)",
            canonical.display()
        ));
    }
    if !canonical.starts_with(jail) {
        return Err(format!(
            "context import {} escapes path jail {}",
            canonical.display(),
            jail.display()
        ));
    }
    if let Some(start) = stack.iter().position(|candidate| candidate == &canonical) {
        let mut cycle = stack[start..]
            .iter()
            .map(|item| item.display().to_string())
            .collect::<Vec<_>>();
        cycle.push(canonical.display().to_string());
        return Err(format!("context import cycle: {}", cycle.join(" -> ")));
    }
    let raw = fs::read_to_string(&canonical)
        .map_err(|error| format!("cannot read context file {}: {error}", canonical.display()))?;
    provenance.push(Provenance {
        path: canonical.clone(),
        imported_by: imported_by.map(Path::to_path_buf),
    });
    stack.push(canonical.clone());
    let mut expanded = String::new();
    for line in raw.split_inclusive('\n') {
        if let Some(import) = parse_import(line.trim_end_matches(['\r', '\n'])) {
            let parent = canonical.parent().unwrap_or(jail);
            let imported = parent.join(import);
            let content = load_expanded(
                &imported,
                jail,
                budget,
                stack,
                provenance,
                Some(&canonical),
            )?;
            expanded.push_str(&content);
            if !content.ends_with('\n') {
                expanded.push('\n');
            }
        } else {
            expanded.push_str(&budget.include(&canonical, line));
        }
    }
    stack.pop();
    Ok(expanded)
}

fn ancestor_chain(root: &Path, cwd: &Path) -> Vec<PathBuf> {
    let mut chain = cwd
        .ancestors()
        .take_while(|path| path.starts_with(root))
        .map(Path::to_path_buf)
        .collect::<Vec<_>>();
    chain.reverse();
    chain
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
    let mut budget = Budget {
        max_bytes: config.max_bytes,
        max_tokens: config.max_tokens,
        used_bytes: 0,
        used_tokens: 0,
        warnings: Vec::new(),
        warned_paths: BTreeSet::new(),
        files_read: 0,
    };
    // Explicit sticky rules are load-bearing policy. Reserve their budget first,
    // while assigning their final precedence after discovered project files.
    let mut configured_entries = Vec::new();
    let mut configured_precedence = 0usize;
    append_configured_rules(
        &root,
        &mut budget,
        &mut configured_entries,
        &mut configured_precedence,
        &rules.always_apply,
    )?;
    let mut entries = Vec::new();
    let mut precedence = 0usize;
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
                precedence,
                content,
                provenance,
            });
            precedence = precedence.saturating_add(1);
        }
    }
    for mut entry in configured_entries {
        entry.precedence = precedence;
        precedence = precedence.saturating_add(1);
        entries.push(entry);
    }
    let mut seen_ids = BTreeSet::new();
    for entry in &entries {
        if !seen_ids.insert(entry.id.clone()) {
            budget
                .warnings
                .push(format!("rule id '{}' is repeated; higher precedence applies later", entry.id));
        }
    }
    Ok(ContextBundle {
        root,
        entries,
        warnings: budget.warnings,
    })
}

fn append_configured_rules(
    jail: &Path,
    budget: &mut Budget,
    entries: &mut Vec<ContextEntry>,
    precedence: &mut usize,
    rules: &[AlwaysApplyRuleConfig],
) -> Result<(), String> {
    for rule in rules {
        let (content, provenance) = match (&rule.content, &rule.source) {
            (Some(content), None) => (
                budget.include(Path::new("<config>"), content),
                vec![Provenance { path: PathBuf::from("<config>"), imported_by: None }],
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
            _ => return Err(format!(
                "always-apply rule '{}' must set exactly one of content or source",
                rule.id
            )),
        };
        entries.push(ContextEntry {
            id: rule.id.clone(),
            kind: ContextKind::Rule,
            always_apply: true,
            precedence: *precedence,
            content,
            provenance,
        });
        *precedence = precedence.saturating_add(1);
    }
    Ok(())
}
