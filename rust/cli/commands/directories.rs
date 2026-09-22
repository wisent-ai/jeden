//! Reading custom slash commands off disk, in each of the three shapes a
//! provider writes them.
//!
//! Split out of `cli/commands/mod.rs`, which had grown past the module line
//! cap; which directories are consulted, and in what order, stays there.

use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};

use super::{valid_file_command_name, DiscoveredCommand};

fn push_discovered_command(
    out: &mut Vec<DiscoveredCommand>,
    seen: &mut BTreeSet<String>,
    name: String,
    path: PathBuf,
    source: &str,
) {
    if valid_file_command_name(&name) && seen.insert(name.clone()) {
        out.push(DiscoveredCommand {
            name,
            path,
            source: source.to_string(),
        });
    }
}

fn md_files(dir: &Path, recursive: bool) -> Vec<PathBuf> {
    let mut out = Vec::new();
    let mut stack = vec![dir.to_path_buf()];
    while let Some(current) = stack.pop() {
        let Ok(entries) = fs::read_dir(&current) else {
            continue;
        };
        let mut paths = entries
            .flatten()
            .map(|entry| entry.path())
            .collect::<Vec<_>>();
        paths.sort();
        for path in paths {
            let name = path.file_name().and_then(|s| s.to_str()).unwrap_or("");
            if name.starts_with('.') {
                continue;
            }
            if recursive && path.is_dir() {
                stack.push(path);
            } else if path.extension().and_then(|e| e.to_str()) == Some("md") {
                out.push(path);
            }
        }
    }
    out
}

fn frontmatter_field(text: &str, key: &str) -> Option<String> {
    let trimmed = text.trim_start_matches("\u{feff}");
    let rest = trimmed.strip_prefix("---\n")?;
    let end = rest.find("\n---")?;
    for line in rest[..end].lines() {
        let Some((field, value)) = line.split_once(':') else {
            continue;
        };
        if field.trim() == key {
            let value = value
                .trim()
                .trim_matches('"')
                .trim_matches('\'')
                .to_string();
            if !value.is_empty() {
                return Some(value);
            }
        }
    }
    None
}

pub(super) fn add_flat_command_dir(
    out: &mut Vec<DiscoveredCommand>,
    seen: &mut BTreeSet<String>,
    dir: PathBuf,
    source: &str,
) {
    for path in md_files(&dir, false) {
        let Some(name) = path
            .file_stem()
            .and_then(|s| s.to_str())
            .map(ToString::to_string)
        else {
            continue;
        };
        push_discovered_command(out, seen, name, path, source);
    }
}

pub(super) fn add_frontmatter_name_dir(
    out: &mut Vec<DiscoveredCommand>,
    seen: &mut BTreeSet<String>,
    dir: PathBuf,
    source: &str,
) {
    for path in md_files(&dir, false) {
        let name = fs::read_to_string(&path)
            .ok()
            .and_then(|text| frontmatter_field(&text, "name"))
            .or_else(|| {
                path.file_stem()
                    .and_then(|s| s.to_str())
                    .map(ToString::to_string)
            });
        let Some(name) = name else { continue };
        push_discovered_command(out, seen, name, path, source);
    }
}

pub(super) fn add_claude_command_dir(
    out: &mut Vec<DiscoveredCommand>,
    seen: &mut BTreeSet<String>,
    dir: PathBuf,
    source: &str,
) {
    for path in md_files(&dir, true) {
        let Some(stem) = path
            .file_stem()
            .and_then(|s| s.to_str())
            .map(ToString::to_string)
        else {
            continue;
        };
        push_discovered_command(out, seen, stem.clone(), path.clone(), source);
        if let Ok(relative) = path.strip_prefix(&dir) {
            let mut parts = relative
                .iter()
                .filter_map(|part| part.to_str())
                .collect::<Vec<_>>();
            if matches!(parts.as_slice(), [_, _, ..]) {
                if let Some(last) = parts.last_mut() {
                    *last = stem.as_str();
                }
                push_discovered_command(out, seen, parts.join(":"), path, source);
            }
        }
    }
}
