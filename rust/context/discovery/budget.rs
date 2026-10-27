//! Keeping the context assembled for a prompt inside the byte budget the
//! operator declared (`context.maxBytes`), and saying so when something was
//! left out. With no declaration every discovered context and rule file is
//! included whole: the model's own context window is what decides, and a
//! provider refuses a prompt past it by name.
//!
//! Split out of `context/discovery.rs`, which had grown past the module line
//! cap.

use std::collections::BTreeSet;
use std::path::Path;
use std::path::PathBuf;

/// A declared byte budget and how much of it is still free.
struct Limit {
    declared: usize,
    remaining: usize,
}

pub(super) struct Budget {
    limit: Option<Limit>,
    warned_paths: BTreeSet<PathBuf>,
    pub(super) warnings: Vec<String>,
}

impl Budget {
    /// A budget of `max_bytes`, all of it free; none declared includes all.
    pub(super) fn new(max_bytes: Option<usize>) -> Self {
        Self {
            limit: max_bytes.map(|declared| Limit {
                declared,
                remaining: declared,
            }),
            warned_paths: BTreeSet::new(),
            warnings: Vec::new(),
        }
    }

    pub(super) fn include(&mut self, path: &Path, text: &str) -> String {
        let Some(limit) = self.limit.as_mut() else {
            return text.to_string();
        };
        let included = match text
            .char_indices()
            .map(|(start, ch)| start + ch.len_utf8())
            .take_while(|end| *end <= limit.remaining)
            .last()
        {
            Some(end) => &text[..end],
            None => "",
        };
        limit.remaining -= included.len();
        if included.len() < text.len() && self.warned_paths.insert(path.to_path_buf()) {
            self.warnings.push(format!(
                "context.maxBytes reached while reading {}: included {} of {} bytes (declared budget {} bytes)",
                path.display(),
                included.len(),
                text.len(),
                limit.declared
            ));
        }
        included.to_string()
    }
}
