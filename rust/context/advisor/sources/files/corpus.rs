//! Reading the declared roots into ranked units.
//!
//! Everything readable counts: documentation, source code, configuration,
//! manifests. A chunk, not a file, is the unit — "read the architecture map"
//! is not a recommendation, while that map's path with the line range of the
//! matching part is. Markdown is cut at its headings because that is where
//! its meaning starts; every other file is cut where a top-level opener —
//! a column-zero line after a blank line — starts, because that is where a
//! declaration or a paragraph begins and a thousand-line source file is not
//! one answer.
//!
//! The walk reads every readable text file under each root, as deep as the
//! root declares and the whole tree when it declares no depth.

use std::fs;
use std::num::NonZeroUsize;
use std::path::{Path, PathBuf};

use ignore::WalkBuilder;

use crate::context::advisor::Settings;

/// A file with a zero byte in it is not text, whatever its name says.
const NUL: char = '\u{0}';

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
                corpus.collect_root(&root.path, root.depth, settings);
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
            "declared roots hold no readable text file within their declared depth".to_string()
        }
    }

    fn collect_root(&mut self, root: &Path, depth: Option<usize>, settings: &Settings) {
        let home = crate::dirs_home();
        let walked_root = root.to_path_buf();
        let walk = WalkBuilder::new(root)
            .max_depth(depth)
            .follow_links(false)
            .filter_entry(move |entry| {
                !crate::tool_runtime::runtime_ops::platform::guarded::walk_skips(
                    &home,
                    &walked_root,
                    entry.path(),
                    entry.file_type().is_some_and(|kind| kind.is_dir()),
                )
            })
            .build();
        for entry in walk.flatten() {
            if !entry.file_type().is_some_and(|kind| kind.is_file()) {
                continue;
            }
            let path = entry.path();
            if !settings.reads_extension(path) {
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
                    self.push(
                        path,
                        &title,
                        first_line,
                        last_line,
                        std::mem::take(&mut body),
                    );
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
        }
        if !body.trim().is_empty() {
            self.push(path, &title, first_line, last_line, body);
        }
    }

    /// Everything else is cut where a top-level opener starts: a line at
    /// column zero after a blank line. In source that is the declaration the
    /// rest of the chunk belongs to, and in prose it is a paragraph's opening;
    /// the opener is the chunk's title.
    fn split_windows(&mut self, path: &Path, text: &str) {
        let lines: Vec<&str> = text.lines().collect();
        let mut first = usize::default();
        for index in lines.iter().enumerate().skip(NonZeroUsize::MIN.get()).filter_map(|(index, line)| {
            let opener = !line.is_empty() && !line.starts_with(char::is_whitespace);
            (opener && lines[index - NonZeroUsize::MIN.get()].trim().is_empty()).then_some(index)
        }).chain(std::iter::once(lines.len())) {
            self.push_window(path, &lines[first..index], first);
            first = index;
        }
    }

    fn push_window(&mut self, path: &Path, window: &[&str], start: usize) {
        let body = window.join("\n");
        if body.trim().is_empty() {
            return;
        }
        let first_line = start + NonZeroUsize::MIN.get();
        let last_line = start + window.len();
        self.push(path, &window_title(path, window), first_line, last_line, body);
    }

    fn push(
        &mut self,
        path: &Path,
        title: &str,
        first_line: usize,
        last_line: usize,
        body: String,
    ) {
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
        Some(line) => line.trim().to_string(),
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
    text.contains(NUL)
}
