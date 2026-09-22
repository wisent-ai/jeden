//! Which files count as context, in which order, and how an import inside one
//! is followed.
//!
//! Split out of `context/discovery.rs`, which had grown past the module line
//! cap.

use super::budget::Budget;
use super::{ContextKind, Provenance};
use std::fs;
use std::path::{Path, PathBuf};

#[derive(Clone, Copy)]
pub(super) struct Descriptor {
    pub(super) relative: &'static str,
    pub(super) kind: ContextKind,
}

// Ordered from lowest to highest precedence within one directory. The array is
// the stable schema descriptor; discovery never depends on directory iteration.
pub(super) const DESCRIPTORS: &[Descriptor] = &[
    Descriptor {
        relative: "CLAUDE.md",
        kind: ContextKind::Rule,
    },
    Descriptor {
        relative: "AGENTS.md",
        kind: ContextKind::Rule,
    },
    Descriptor {
        relative: "RULES.md",
        kind: ContextKind::Rule,
    },
    Descriptor {
        relative: "JEDEN.md",
        kind: ContextKind::Rule,
    },
    Descriptor {
        relative: ".jeden/instructions.md",
        kind: ContextKind::Rule,
    },
    Descriptor {
        relative: ".jeden/context.md",
        kind: ContextKind::Context,
    },
];

fn parse_import(line: &str) -> Option<&str> {
    let trimmed = line.trim();
    let raw = trimmed
        .strip_prefix("@import ")
        .or_else(|| trimmed.strip_prefix("@include "))
        .or_else(|| {
            trimmed
                .strip_prefix('@')
                .filter(|rest| !rest.contains(char::is_whitespace))
        })?;
    let path = raw.trim().trim_matches(['\'', '"']);
    (!path.is_empty()).then_some(path)
}

pub(super) fn load_expanded(
    path: &Path,
    jail: &Path,
    budget: &mut Budget,
    stack: &mut Vec<PathBuf>,
    provenance: &mut Vec<Provenance>,
    imported_by: Option<&Path>,
) -> Result<String, String> {
    let canonical = path.canonicalize().map_err(|error| {
        format!(
            "context import {} cannot be resolved: {error}",
            path.display()
        )
    })?;
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
            let content =
                load_expanded(&imported, jail, budget, stack, provenance, Some(&canonical))?;
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

pub(super) fn ancestor_chain(root: &Path, cwd: &Path) -> Vec<PathBuf> {
    let mut chain = cwd
        .ancestors()
        .take_while(|path| path.starts_with(root))
        .map(Path::to_path_buf)
        .collect::<Vec<_>>();
    chain.reverse();
    chain
}
