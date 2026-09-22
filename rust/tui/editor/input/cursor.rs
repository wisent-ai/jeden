//! Moving through text the way a person reads it rather than the way it is
//! stored.
//!
//! Split out of `tui/editor/mod.rs`, which had grown past the module line cap.

use std::cmp::Ordering;
use unicode_segmentation::UnicodeSegmentation;
use unicode_width::UnicodeWidthStr;

pub(crate) fn ordered(a: usize, b: usize) -> (usize, usize) {
    match a.cmp(&b) {
        Ordering::Less | Ordering::Equal => (a, b),
        Ordering::Greater => (b, a),
    }
}

pub(crate) fn previous_boundary(text: &str, cursor: usize) -> usize {
    text[..cursor]
        .grapheme_indices(true)
        .next_back()
        .map_or(0, |(index, _)| index)
}

pub(crate) fn next_boundary(text: &str, cursor: usize) -> usize {
    text[cursor..]
        .grapheme_indices(true)
        .nth(1)
        .map_or(text.len(), |(index, _)| cursor + index)
}

pub(crate) fn line_start(text: &str, cursor: usize) -> usize {
    text[..cursor].rfind('\n').map_or(0, |index| index + 1)
}

pub(crate) fn line_end(text: &str, cursor: usize) -> usize {
    text[cursor..]
        .find('\n')
        .map_or(text.len(), |index| cursor + index)
}

pub(crate) fn word_left(text: &str, cursor: usize) -> usize {
    let before = &text[..cursor];
    let mut target = 0;
    let mut seen_word = false;
    for (index, grapheme) in before.grapheme_indices(true).rev() {
        let word = grapheme.chars().any(char::is_alphanumeric) || grapheme == "_";
        if word {
            seen_word = true;
            target = index;
        } else if seen_word {
            break;
        } else {
            target = index;
        }
    }
    target
}

pub(crate) fn word_right(text: &str, cursor: usize) -> usize {
    let mut seen_word = false;
    for (offset, grapheme) in text[cursor..].grapheme_indices(true) {
        let word = grapheme.chars().any(char::is_alphanumeric) || grapheme == "_";
        if word {
            seen_word = true;
        } else if seen_word {
            return cursor + offset;
        }
    }
    text.len()
}

pub(crate) fn byte_at_display_column(text: &str, start: usize, end: usize, target: usize) -> usize {
    let mut width = 0;
    let mut byte = start;
    for (offset, grapheme) in text[start..end].grapheme_indices(true) {
        let next = width + UnicodeWidthStr::width(grapheme);
        if next > target {
            break;
        }
        width = next;
        byte = start + offset + grapheme.len();
    }
    byte
}

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
            ch if !ch.is_control() && !matches!(ch as u32, 0x80..=0x9f) => normalized.push(ch),
            _ => {}
        }
    }
    normalized
}
