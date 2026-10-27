//! Reading the declared roots into ranked units.
//!
//! Everything readable counts: documentation, source code, configuration,
//! manifests. A chunk, not a file, is the unit — "read the architecture map"
//! is not a recommendation, while that map's path with the line range of the
//! matching part is. Markdown is cut at its headings because that is where
//! its meaning starts; every other file is cut where a top-level opener —
//! a column-zero line after a blank line — starts. Those sections retain the
//! file name as their title: unparsed content is not title metadata and must
//! not receive a second ranking weight merely by opening a section.
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

#[derive(Default)]
pub(super) struct Corpus {
    pub(super) sections: Vec<Section>,
    pub(super) files: usize,
    pub(super) existing_roots: Vec<String>,
    pub(super) missing_roots: Vec<String>,
    /// Files and directories under the roots that could not be read, as
    /// `path: cause`; their text is in no section.
    pub(super) unreadable: Vec<String>,
}

impl Corpus {
    pub(super) fn read(
        settings: &Settings,
        cancellation: &crate::tool_runtime::runtime_ops::CancellationToken,
    ) -> Self {
        let mut corpus = Self::default();
        for root in &settings.roots {
            if cancellation.is_cancelled() {
                break;
            }
            match root.path.try_exists() {
                Ok(true) => {
                    corpus.existing_roots.push(root.path.display().to_string());
                    corpus.collect_root(&root.path, root.depth, settings, cancellation);
                }
                Ok(false) => corpus.missing_roots.push(root.path.display().to_string()),
                Err(error) => corpus.unreadable.push(format!(
                    "inspect declared root {}: {error}",
                    root.path.display()
                )),
            }
        }
        corpus
    }

    /// Why the corpus is empty, in the words an operator can act on.
    pub(super) fn empty_detail(&self) -> String {
        if !self.unreadable.is_empty() {
            return format!(
                "declared files source could not be read: {}",
                self.unreadable.join("; ")
            );
        }
        if self.existing_roots.is_empty() {
            format!("no declared root exists: {}", self.missing_roots.join(", "))
        } else {
            "declared roots hold no readable text file within their declared depth".to_string()
        }
    }

    fn collect_root(
        &mut self,
        root: &Path,
        depth: Option<usize>,
        settings: &Settings,
        cancellation: &crate::tool_runtime::runtime_ops::CancellationToken,
    ) {
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
        for entry in walk {
            let entry = match entry {
                Ok(entry) => entry,
                Err(error) => {
                    self.unreadable.push(error.to_string());
                    continue;
                }
            };
            if cancellation.is_cancelled() {
                break;
            }
            if !entry.file_type().is_some_and(|kind| kind.is_file()) {
                continue;
            }
            let path = entry.path();
            if !settings.reads_extension(path) {
                continue;
            }
            // Text that is not UTF-8 is binary and skipped like the rest;
            // any other failure is a file whose text is missing.
            let text = match fs::read_to_string(path) {
                Ok(text) => text,
                Err(error) if error.kind() == std::io::ErrorKind::InvalidData => continue,
                Err(error) => {
                    self.unreadable.push(format!("{}: {error}", path.display()));
                    continue;
                }
            };
            if is_binary(&text) {
                continue;
            }
            let title = match file_title(path) {
                Ok(title) => title,
                Err(error) => {
                    self.unreadable.push(error);
                    continue;
                }
            };
            self.files += NonZeroUsize::MIN.get();
            if markdown(path) {
                self.split_headings(path, &title, &text);
            } else {
                self.split_windows(path, &title, &text);
            }
        }
    }

    /// Markdown headings start a section; a file without one is a single
    /// section named after the file, because "no heading" is not "not worth
    /// reading".
    fn split_headings(&mut self, path: &Path, file_title: &str, text: &str) {
        let mut title = file_title.to_string();
        let mut first_line = NonZeroUsize::MIN.get();
        let mut last_line = first_line;
        let mut body = String::new();
        for (number, line) in (NonZeroUsize::MIN.get()..).zip(text.lines()) {
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
                    title = file_title.to_string();
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
    /// column zero after a blank line. The file's identity stays the title
    /// rather than an arbitrary source or data line promoted to metadata.
    fn split_windows(&mut self, path: &Path, title: &str, text: &str) {
        let mut window: Vec<&str> = Vec::new();
        let mut start = NonZeroUsize::MIN.get();
        for (number, line) in (NonZeroUsize::MIN.get()..).zip(text.lines()) {
            let opens = !line.is_empty()
                && !line.starts_with(char::is_whitespace)
                && window.last().is_some_and(|previous| previous.trim().is_empty());
            if opens {
                self.push_window(path, title, &window, start);
                window.clear();
                start = number;
            }
            window.push(line);
        }
        self.push_window(path, title, &window, start);
    }

    fn push_window(&mut self, path: &Path, title: &str, window: &[&str], first_line: usize) {
        let body = window.join("\n");
        if body.trim().is_empty() {
            return;
        }
        let last_line = first_line + window.len() - NonZeroUsize::MIN.get();
        self.push(path, title, first_line, last_line, body);
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

/// The walked file's own name; a path the walk yields without one is an
/// unreadable entry, named with its path, rather than a section given an
/// invented title.
fn file_title(path: &Path) -> Result<String, String> {
    path.file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .ok_or_else(|| {
            format!(
                "{}: a walked file has no file name to title its sections",
                path.display()
            )
        })
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
