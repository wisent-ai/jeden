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

pub(super) struct Budget {
    pub(super) max_bytes: Option<usize>,
    pub(super) used_bytes: usize,
    pub(super) warned_paths: BTreeSet<PathBuf>,
    pub(super) warnings: Vec<String>,
}

impl Budget {
    pub(super) fn include(&mut self, path: &Path, text: &str) -> String {
        let Some(max_bytes) = self.max_bytes else {
            return text.to_string();
        };
        let remaining = max_bytes.saturating_sub(self.used_bytes);
        let included = match text
            .char_indices()
            .map(|(start, ch)| start + ch.len_utf8())
            .take_while(|end| *end <= remaining)
            .last()
        {
            Some(end) => &text[..end],
            None => "",
        };
        self.used_bytes = self.used_bytes.saturating_add(included.len());
        if included.len() < text.len() && self.warned_paths.insert(path.to_path_buf()) {
            self.warnings.push(format!(
                "context.maxBytes reached while reading {}: included {} of {} bytes (declared budget {} bytes)",
                path.display(),
                included.len(),
                text.len(),
                max_bytes
            ));
        }
        included.to_string()
    }
}
