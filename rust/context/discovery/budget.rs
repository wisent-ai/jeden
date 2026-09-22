//! Keeping the context assembled for a prompt inside what the model can be
//! given, and saying so when something was left out.
//!
//! Split out of `context/discovery.rs`, which had grown past the module line
//! cap.

use std::collections::BTreeSet;
use std::path::Path;
use std::path::PathBuf;

pub(super) struct Budget {
    pub(super) max_bytes: usize,
    pub(super) max_tokens: usize,
    pub(super) used_bytes: usize,
    pub(super) used_tokens: usize,
    pub(super) warned_paths: BTreeSet<PathBuf>,
    pub(super) files_read: usize,
    pub(super) warnings: Vec<String>,
}

impl Budget {
    pub(super) fn include(&mut self, path: &Path, text: &str) -> String {
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
