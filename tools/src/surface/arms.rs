//! Reading the arms of a Rust match block: where each arm begins, and the
//! literals in its pattern. This is how the command vocabulary is recovered
//! from the dispatcher itself rather than from a list somebody has to
//! remember to update.

use super::source::Source;

fn opens(byte: u8) -> bool {
    matches!(byte, b'{' | b'(' | b'[')
}

fn closes(byte: u8) -> bool {
    matches!(byte, b'}' | b')' | b']')
}

/// Literals in the pattern of every arm of the match block spanning
/// `start..stop`.
///
/// Arms are split at `=>` found at the block's own nesting depth, and only
/// literals at that same depth inside a pattern are taken. A literal in an arm
/// body is nested inside the body's braces or parentheses, so it cannot be
/// mistaken for a pattern.
pub(crate) fn match_arm_patterns(
    source: &Source,
    start: usize,
    stop: usize,
) -> Result<Vec<String>, String> {
    let mask = source.mask.as_bytes();
    let mut patterns = Vec::new();
    let mut depth = 0isize;
    let mut boundary = start;
    let mut index = start;
    while index < stop {
        let byte = mask[index];
        if opens(byte) {
            depth += 1;
            index += 1;
            continue;
        }
        if closes(byte) {
            depth -= 1;
            index += 1;
            continue;
        }
        if depth == 0 && mask[index..].starts_with(b"=>") {
            patterns.push((boundary, index));
            let mut body = index + 2;
            while body < stop && mask[body] == b' ' {
                body += 1;
            }
            if body < stop && mask[body] == b'{' {
                body = source.balanced_end(body, b'{', b'}')?;
                while body < stop && matches!(mask[body], b' ' | b',' | b'\n') {
                    body += 1;
                }
                boundary = body;
                index = body;
                continue;
            }
            let mut inner = 0isize;
            while body < stop {
                let token = mask[body];
                if opens(token) {
                    inner += 1;
                } else if closes(token) {
                    inner -= 1;
                } else if token == b',' && inner == 0 {
                    break;
                }
                body += 1;
            }
            boundary = body + 1;
            index = boundary;
            continue;
        }
        index += 1;
    }
    let mut names = Vec::new();
    for (begin, end) in patterns {
        for (offset, _, value) in &source.literals {
            if (begin..end).contains(offset) && pattern_depth(mask, begin, *offset) == 0 {
                names.push(value.clone());
            }
        }
    }
    Ok(names)
}

fn pattern_depth(mask: &[u8], begin: usize, offset: usize) -> isize {
    mask[begin..offset].iter().fold(0, |depth, &byte| {
        if opens(byte) {
            depth + 1
        } else if closes(byte) {
            depth - 1
        } else {
            depth
        }
    })
}
