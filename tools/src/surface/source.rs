//! One Rust file, scanned once into its string literals and a masked copy.
//!
//! The mask is the file with comment bodies, character literals and string
//! contents blanked to spaces, keeping the quotes and every newline. Offsets
//! stay aligned with the original, so braces can be matched without a `{`
//! inside a string throwing the depth off, and a search for a declaration
//! cannot match something only ever mentioned in a comment or a string.

use regex::{Match, Regex};
use std::fs;
use std::path::Path;

pub(crate) struct Source {
    label: String,
    text: Vec<u8>,
    pub(crate) mask: String,
    /// `(start, end, value)` for every string literal, `start` at its prefix
    /// or opening quote.
    pub(crate) literals: Vec<(usize, usize, String)>,
}

fn ident(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || byte == b'_'
}

fn escape(byte: u8) -> u8 {
    match byte {
        b'n' => b'\n',
        b't' => b'\t',
        b'r' => b'\r',
        b'0' => b'\0',
        other => other,
    }
}

impl Source {
    pub(crate) fn read(path: &Path, label: &str) -> Result<Self, String> {
        let text = fs::read(path).map_err(|error| format!("cannot read {label}: {error}"))?;
        let mut source = Self {
            label: label.into(),
            text,
            mask: String::new(),
            literals: Vec::new(),
        };
        source.mask = source.scan()?;
        Ok(source)
    }

    fn error(&self, what: &str) -> String {
        format!("{}: {what}", self.label)
    }

    fn scan(&mut self) -> Result<String, String> {
        let size = self.text.len();
        let mut mask = self.text.clone();
        let mut index = 0;
        while index < size {
            let byte = self.text[index];
            let following = self.text.get(index + 1).copied();
            if byte == b'/' && following == Some(b'/') {
                let stop = self.text[index..]
                    .iter()
                    .position(|&candidate| candidate == b'\n')
                    .map_or(size, |offset| index + offset);
                index = blank(&mut mask, index, stop);
            } else if byte == b'/' && following == Some(b'*') {
                let stop = self.block_comment_end(index)?;
                index = blank(&mut mask, index, stop);
            } else if byte == b'\'' {
                let stop = self.quote_or_lifetime_end(index)?;
                index = blank(&mut mask, index, stop);
            } else if let Some((open_end, close_start, close_end)) = self.raw_string_span(index)? {
                let value = String::from_utf8_lossy(&self.text[open_end..close_start]).into_owned();
                self.literals.push((index, close_end, value));
                blank(&mut mask, open_end, close_start);
                index = close_end;
            } else if byte == b'"' {
                index = self.plain_string(&mut mask, index)?;
            } else {
                index += 1;
            }
        }
        String::from_utf8(mask).map_err(|error| self.error(&error.to_string()))
    }

    fn block_comment_end(&self, start: usize) -> Result<usize, String> {
        let mut depth = 1;
        let mut index = start + 2;
        while index < self.text.len() && depth > 0 {
            if self.text[index..].starts_with(b"/*") {
                depth += 1;
                index += 2;
            } else if self.text[index..].starts_with(b"*/") {
                depth -= 1;
                index += 2;
            } else {
                index += 1;
            }
        }
        if depth > 0 {
            return Err(self.error("unterminated block comment"));
        }
        Ok(index)
    }

    /// End of a char literal, or of a lifetime such as `'static`.
    fn quote_or_lifetime_end(&self, start: usize) -> Result<usize, String> {
        let size = self.text.len();
        let index = start + 1;
        if self.text.get(index) == Some(&b'\\') {
            let mut end = index + 2;
            while end < size && self.text[end] != b'\'' {
                end += 1;
            }
            if end >= size {
                return Err(self.error("unterminated char literal"));
            }
            return Ok(end + 1);
        }
        let mut run = index;
        while run < size && ident(self.text[run]) {
            run += 1;
        }
        if run == index + 1 && self.text.get(run) == Some(&b'\'') {
            return Ok(run + 1);
        }
        Ok(run)
    }

    /// `(open_end, close_start, close_end)` of a raw or byte-raw string
    /// starting at `start`.
    fn raw_string_span(&self, start: usize) -> Result<Option<(usize, usize, usize)>, String> {
        if start > 0 && ident(self.text[start - 1]) {
            return Ok(None);
        }
        let mut index = start;
        if self.text.get(index) == Some(&b'b') {
            index += 1;
        }
        if self.text.get(index) != Some(&b'r') {
            return Ok(None);
        }
        index += 1;
        let mut hashes = 0;
        while self.text.get(index) == Some(&b'#') {
            hashes += 1;
            index += 1;
        }
        if self.text.get(index) != Some(&b'"') {
            return Ok(None);
        }
        let open_end = index + 1;
        let mut terminator = vec![b'"'];
        terminator.extend(std::iter::repeat(b'#').take(hashes));
        let close_start = self.text[open_end..]
            .windows(terminator.len())
            .position(|window| window == terminator.as_slice())
            .map(|offset| open_end + offset)
            .ok_or_else(|| self.error("unterminated raw string"))?;
        Ok(Some((
            open_end,
            close_start,
            close_start + terminator.len(),
        )))
    }

    fn plain_string(&mut self, mask: &mut [u8], start: usize) -> Result<usize, String> {
        let size = self.text.len();
        let mut index = start + 1;
        let mut value = Vec::new();
        while index < size {
            let byte = self.text[index];
            if byte == b'\\' {
                let Some(&next) = self.text.get(index + 1) else {
                    break;
                };
                value.push(escape(next));
                index += 2;
                continue;
            }
            if byte == b'"' {
                let text = String::from_utf8_lossy(&value).into_owned();
                self.literals.push((start, index + 1, text));
                blank(mask, start + 1, index);
                return Ok(index + 1);
            }
            value.push(byte);
            index += 1;
        }
        Err(self.error("unterminated string literal"))
    }

    pub(crate) fn literal_at(&self, offset: usize) -> Result<String, String> {
        self.literals
            .iter()
            .find(|(start, _, _)| *start == offset)
            .map(|(_, _, value)| value.clone())
            .ok_or_else(|| self.error(&format!("expected a string literal at offset {offset}")))
    }

    pub(crate) fn literals_within(&self, start: usize, stop: usize) -> Vec<String> {
        self.literals
            .iter()
            .filter(|(begin, _, _)| (start..stop).contains(begin))
            .map(|(_, _, value)| value.clone())
            .collect()
    }

    pub(crate) fn anchors(&self, pattern: &str) -> Result<Vec<Match<'_>>, String> {
        let expression = Regex::new(pattern).map_err(|error| error.to_string())?;
        Ok(expression.find_iter(&self.mask).collect())
    }

    pub(crate) fn sole_anchor(&self, pattern: &str, what: &str) -> Result<Match<'_>, String> {
        let found = self.anchors(pattern)?;
        match found.as_slice() {
            [only] => Ok(*only),
            _ => Err(self.error(&format!(
                "expected exactly one {what}, found {}",
                found.len()
            ))),
        }
    }

    /// Offset just past the `closer` matching the `opener` at `start`.
    pub(crate) fn balanced_end(
        &self,
        start: usize,
        opener: u8,
        closer: u8,
    ) -> Result<usize, String> {
        let mask = self.mask.as_bytes();
        if mask.get(start) != Some(&opener) {
            return Err(self.error(&format!("expected {:?} at offset {start}", opener as char)));
        }
        let mut depth = 0usize;
        for (index, &byte) in mask.iter().enumerate().skip(start) {
            if byte == opener {
                depth += 1;
            } else if byte == closer {
                depth -= 1;
                if depth == 0 {
                    return Ok(index + 1);
                }
            }
        }
        Err(self.error(&format!(
            "unbalanced {:?} at offset {start}",
            opener as char
        )))
    }
}

/// Blank `start..stop` of the mask to spaces, keeping newlines; returns `stop`.
fn blank(mask: &mut [u8], start: usize, stop: usize) -> usize {
    for byte in &mut mask[start..stop] {
        if *byte != b'\n' {
            *byte = b' ';
        }
    }
    stop
}
