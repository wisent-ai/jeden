//! Reading the declared roots into ranked units.
//!
//! A section, not a file, is the unit: "read the architecture map" is not a
//! recommendation, while that map's path with the line range of the matching
//! section is. Which file types count as documentation is operator
//! configuration (`context.advisor.docExtensions`), not a judgement this
//! module makes.

use std::fs;
use std::path::{Path, PathBuf};

use ignore::WalkBuilder;

use crate::context::advisor::Settings;

const MAX_FILES: usize = 4_000;
const MAX_FILE_BYTES: u64 = 262_144;
const MAX_SECTIONS: usize = 60_000;

pub(super) struct Section {
    pub(super) path: PathBuf,
    pub(super) title: String,
    pub(super) first_line: usize,
    pub(super) last_line: usize,
    pub(super) body: String,
}

impl Section {
    /// Whether the term or its stem occurs anywhere in the section, which is
    /// what the term's corpus frequency is counted over.
    pub(super) fn contains(&self, term: &str) -> bool {
        let stemmed = crate::context::advisor::text::stem(term);
        let haystacks = [
            self.body.to_lowercase(),
            self.title.to_lowercase(),
            self.path.display().to_string().to_lowercase(),
        ];
        haystacks
            .iter()
            .any(|field| field.contains(term) || field.contains(stemmed.as_str()))
    }
}

pub(super) struct Corpus {
    pub(super) sections: Vec<Section>,
    pub(super) files: usize,
    pub(super) existing_roots: Vec<String>,
    pub(super) missing_roots: Vec<String>,
}

impl Corpus {
    pub(super) fn read(settings: &Settings) -> Self {
        let mut corpus = Self {
            sections: Vec::new(),
            files: usize::default(),
            existing_roots: Vec::new(),
            missing_roots: Vec::new(),
        };
        for root in &settings.roots {
            if root.path.exists() {
                corpus.existing_roots.push(root.path.display().to_string());
                corpus.collect_root(&root.path, root.depth, &settings.doc_extensions);
            } else {
                corpus.missing_roots.push(root.path.display().to_string());
            }
        }
        corpus
    }

    /// Why the corpus is empty, in the words an operator can act on.
    pub(super) fn empty_detail(&self) -> String {
        if self.existing_roots.is_empty() {
            format!("no declared root exists: {}", self.missing_roots.join(", "))
        } else {
            "declared roots hold no documentation file within their depth limit".to_string()
        }
    }

    fn collect_root(&mut self, root: &Path, depth: usize, extensions: &[String]) {
        let walk = WalkBuilder::new(root)
            .max_depth(Some(depth))
            .follow_links(false)
            .build();
        for entry in walk.flatten() {
            if self.files >= MAX_FILES || self.sections.len() >= MAX_SECTIONS {
                return;
            }
            if !entry.file_type().is_some_and(|kind| kind.is_file()) {
                continue;
            }
            let path = entry.path();
            if !has_declared_extension(path, extensions) {
                continue;
            }
            if entry
                .metadata()
                .map(|meta| meta.len() > MAX_FILE_BYTES)
                .unwrap_or(true)
            {
                continue;
            }
            let Ok(text) = fs::read_to_string(path) else {
                continue;
            };
            self.files += 1;
            self.split_sections(path, &text);
        }
    }

    /// Markdown headings start a section; a file without one is a single
    /// section named after the file, because "no heading" is not "not worth
    /// reading".
    fn split_sections(&mut self, path: &Path, text: &str) {
        let file_title = path
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("document")
            .to_string();
        let mut title = file_title.clone();
        let mut first_line = usize::default() + 1;
        let mut last_line = usize::default();
        let mut body = String::new();
        for (index, line) in text.lines().enumerate() {
            let number = index + 1;
            if line.starts_with('#') {
                if !body.trim().is_empty() {
                    self.sections.push(Section {
                        path: path.to_path_buf(),
                        title: title.clone(),
                        first_line,
                        last_line: last_line.max(first_line),
                        body: std::mem::take(&mut body),
                    });
                }
                title = line.trim_start_matches('#').trim().to_string();
                if title.is_empty() {
                    title = file_title.clone();
                }
                first_line = number;
                body.clear();
            }
            body.push_str(line);
            body.push('\n');
            last_line = number;
            if self.sections.len() >= MAX_SECTIONS {
                return;
            }
        }
        if !body.trim().is_empty() {
            self.sections.push(Section {
                path: path.to_path_buf(),
                title,
                first_line,
                last_line: last_line.max(first_line),
                body,
            });
        }
    }
}

fn has_declared_extension(path: &Path, extensions: &[String]) -> bool {
    let Some(found) = path
        .extension()
        .and_then(|extension| extension.to_str())
        .map(|extension| extension.to_ascii_lowercase())
    else {
        return false;
    };
    extensions.iter().any(|declared| *declared == found)
}
