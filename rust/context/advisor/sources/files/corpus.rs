//! Reading the declared roots into ranked units.
//!
//! Everything readable counts: documentation, source code, configuration,
//! manifests. A chunk, not a file, is the unit — "read the architecture map"
//! is not a recommendation, while that map's path with the line range of the
//! matching part is. Markdown is cut at its headings because that is where
//! its meaning starts; every other file is cut into bounded line windows,
//! because a thousand-line source file is not one answer.
//!
//! The walk is bounded three ways: the depth each root declares, the caps
//! below, and the deadline the caller passes. A root nobody sized cannot
//! stall a turn; it can only produce a short corpus that says it was cut.

use std::fs;
use std::path::{Path, PathBuf};
use std::time::Instant;

use ignore::WalkBuilder;

use crate::context::advisor::Settings;

const MAX_FILES: usize = 20_000;
const MAX_FILE_BYTES: u64 = 262_144;
const MAX_CHUNKS: usize = 120_000;
/// Lines per chunk of a file that carries no headings. Long enough to hold a
/// function with its signature, short enough that a locator points at
/// something a reader can take in.
const CHUNK_LINES: usize = 40;
const MAX_TITLE_CHARS: usize = 120;
/// A file with a zero byte in its head is not text, whatever its name says.
const BINARY_PROBE_BYTES: usize = 8_192;

pub(super) struct Section {
    pub(super) path: PathBuf,
    pub(super) title: String,
    pub(super) first_line: usize,
    pub(super) last_line: usize,
    pub(super) body: String,
    /// The same three fields lowercased once. Matching is case-insensitive
    /// and every term rescans every chunk, so lowercasing per term per chunk
    /// was the whole cost of a large corpus.
    pub(super) lower_body: String,
    pub(super) lower_title: String,
    pub(super) lower_path: String,
}

impl Section {
    /// Whether the term or its stem occurs anywhere in the section, which is
    /// what the term's corpus frequency is counted over.
    pub(super) fn contains(&self, term: &str, stem: &str) -> bool {
        self.lower_body.contains(term)
            || self.lower_title.contains(term)
            || self.lower_path.contains(term)
            || self.lower_body.contains(stem)
            || self.lower_title.contains(stem)
            || self.lower_path.contains(stem)
    }
}

pub(super) struct Corpus {
    pub(super) sections: Vec<Section>,
    pub(super) files: usize,
    pub(super) existing_roots: Vec<String>,
    pub(super) missing_roots: Vec<String>,
    /// True when a cap or the deadline stopped the walk before the roots ran
    /// out, so a short answer is never reported as an exhaustive one.
    pub(super) truncated: bool,
}

impl Corpus {
    pub(super) fn read(settings: &Settings, deadline: Instant) -> Self {
        let mut corpus = Self {
            sections: Vec::new(),
            files: usize::default(),
            existing_roots: Vec::new(),
            missing_roots: Vec::new(),
            truncated: false,
        };
        for root in &settings.roots {
            if root.path.exists() {
                corpus.existing_roots.push(root.path.display().to_string());
                corpus.collect_root(&root.path, root.depth, settings, deadline);
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
            "declared roots hold no readable text file within their depth limit".to_string()
        }
    }

    fn collect_root(
        &mut self,
        root: &Path,
        depth: usize,
        settings: &Settings,
        deadline: Instant,
    ) {
        let walk = WalkBuilder::new(root)
            .max_depth(Some(depth))
            .follow_links(false)
            .build();
        for entry in walk.flatten() {
            if self.files >= MAX_FILES || self.sections.len() >= MAX_CHUNKS {
                self.truncated = true;
                return;
            }
            if Instant::now() >= deadline {
                self.truncated = true;
                return;
            }
            if !entry.file_type().is_some_and(|kind| kind.is_file()) {
                continue;
            }
            let path = entry.path();
            if !settings.reads_extension(path) {
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
            if is_binary(&text) {
                continue;
            }
            self.files += 1;
            if markdown(path) {
                self.split_headings(path, &text);
            } else {
                self.split_windows(path, &text);
            }
        }
    }

    /// Markdown headings start a section; a file without one is a single
    /// section named after the file, because "no heading" is not "not worth
    /// reading".
    fn split_headings(&mut self, path: &Path, text: &str) {
        let file_title = file_title(path);
        let mut title = file_title.clone();
        let mut first_line = usize::default() + 1;
        let mut last_line = usize::default();
        let mut body = String::new();
        for (index, line) in text.lines().enumerate() {
            let number = index + 1;
            if line.starts_with('#') {
                if !body.trim().is_empty() {
                    self.push(path, &title, first_line, last_line, std::mem::take(&mut body));
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
            if self.sections.len() >= MAX_CHUNKS {
                self.truncated = true;
                return;
            }
        }
        if !body.trim().is_empty() {
            self.push(path, &title, first_line, last_line, body);
        }
    }

    /// Everything else is cut into line windows. The title is the window's
    /// first line at column zero — in source that is the declaration the rest
    /// of the window belongs to, and in prose it is the paragraph's opening.
    fn split_windows(&mut self, path: &Path, text: &str) {
        let lines: Vec<&str> = text.lines().collect();
        for (index, window) in lines.chunks(CHUNK_LINES).enumerate() {
            if self.sections.len() >= MAX_CHUNKS {
                self.truncated = true;
                return;
            }
            let body = window.join("\n");
            if body.trim().is_empty() {
                continue;
            }
            let first_line = index * CHUNK_LINES + 1;
            let last_line = first_line + window.len() - 1;
            self.push(path, &window_title(path, window), first_line, last_line, body);
        }
    }

    fn push(&mut self, path: &Path, title: &str, first_line: usize, last_line: usize, body: String) {
        self.sections.push(Section {
            lower_body: body.to_lowercase(),
            lower_title: title.to_lowercase(),
            lower_path: path.display().to_string().to_lowercase(),
            path: path.to_path_buf(),
            title: title.to_string(),
            first_line,
            last_line: last_line.max(first_line),
            body,
        });
    }
}

fn file_title(path: &Path) -> String {
    path.file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("document")
        .to_string()
}

fn window_title(path: &Path, window: &[&str]) -> String {
    let opener = window
        .iter()
        .find(|line| !line.is_empty() && !line.starts_with(char::is_whitespace))
        .or_else(|| window.iter().find(|line| !line.trim().is_empty()));
    match opener {
        Some(line) => line.trim().chars().take(MAX_TITLE_CHARS).collect(),
        None => file_title(path),
    }
}

fn markdown(path: &Path) -> bool {
    matches!(
        path.extension()
            .and_then(|extension| extension.to_str())
            .map(str::to_ascii_lowercase)
            .as_deref(),
        Some("md") | Some("mdx") | Some("markdown")
    )
}

fn is_binary(text: &str) -> bool {
    text.chars()
        .take(BINARY_PROBE_BYTES)
        .any(|character| character == '\u{0}')
}
