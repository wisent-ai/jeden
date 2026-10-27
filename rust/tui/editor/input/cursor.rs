//! Moving through text the way a person reads it rather than the way it is
//! stored.
//!
//! Split out of `tui/editor/mod.rs`, which had grown past the module line cap.

use std::cmp::Ordering;
use unicode_segmentation::UnicodeSegmentation;
use unicode_width::UnicodeWidthStr;

pub(crate) fn ordered(a: usize, b: usize) -> (usize, usize) {
    match a.cmp(&b) {
        Ordering::Greater => (b, a),
        Ordering::Less | Ordering::Equal => (a, b),
    }
}

/// Where the grapheme before `cursor` starts; `cursor` itself at the start of the text.
pub(crate) fn previous_boundary(text: &str, cursor: usize) -> usize {
    match text[..cursor].grapheme_indices(true).next_back() {
        Some((index, _)) => index,
        None => cursor,
    }
}

/// Where the grapheme at `cursor` ends; `cursor` itself at the end of the text.
pub(crate) fn next_boundary(text: &str, cursor: usize) -> usize {
    match text[cursor..].graphemes(true).next() {
        Some(grapheme) => cursor + grapheme.len(),
        None => cursor,
    }
}

/// Where the line holding `cursor` starts.
pub(crate) fn line_start(text: &str, cursor: usize) -> usize {
    let before = &text[..cursor];
    let line = match before.rsplit_once('\n') {
        Some((_, line)) => line,
        None => before,
    };
    cursor - line.len()
}

/// Where the line holding `cursor` ends, before its line break.
pub(crate) fn line_end(text: &str, cursor: usize) -> usize {
    match text[cursor..].find('\n') {
        Some(index) => cursor + index,
        None => text.len(),
    }
}

fn is_word(grapheme: &str) -> bool {
    grapheme.chars().any(char::is_alphanumeric) || grapheme == "_"
}

/// Where the word before `cursor` starts, past any gap between them.
pub(crate) fn word_left(text: &str, cursor: usize) -> usize {
    let mut target = cursor;
    let mut seen_word = false;
    for (index, grapheme) in text[..cursor].grapheme_indices(true).rev() {
        if is_word(grapheme) {
            seen_word = true;
        } else if seen_word {
            break;
        }
        target = index;
    }
    target
}

/// Where the word at or after `cursor` ends.
pub(crate) fn word_right(text: &str, cursor: usize) -> usize {
    let mut seen_word = false;
    for (offset, grapheme) in text[cursor..].grapheme_indices(true) {
        if is_word(grapheme) {
            seen_word = true;
        } else if seen_word {
            return cursor + offset;
        }
    }
    text.len()
}

/// The byte in `start..end` where the text reaches display column `target`:
/// the end of the last grapheme that still fits within it.
pub(crate) fn byte_at_display_column(text: &str, start: usize, end: usize, target: usize) -> usize {
    let mut byte = start;
    for (offset, grapheme) in text[start..end].grapheme_indices(true) {
        let through = start + offset + grapheme.len();
        if UnicodeWidthStr::width(&text[start..through]) > target {
            break;
        }
        byte = through;
    }
    byte
}

/// Pasted text as the editor holds it: line breaks as `\n`, tabs kept, and
/// every other control character (C0 and C1 alike) dropped.
pub(crate) fn normalize_paste(value: &str) -> String {
    let mut normalized = String::with_capacity(value.len());
    let mut chars = value.chars().peekable();
    while let Some(ch) = chars.next() {
        match ch {
            '\r' => {
                if chars.peek() == Some(&'\n') {
                    chars.next();
                }
                normalized.push('\n');
            }
            '\n' | '\t' => normalized.push(ch),
            ch if !ch.is_control() => normalized.push(ch),
            _ => {}
        }
    }
    normalized
}
