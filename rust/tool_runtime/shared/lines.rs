//! Line ranges a tool reads by, and the line-level changes it reports.

use crate::tally::{FIRST, STEP};
use serde_json::{json, Value};
use std::num::NonZeroUsize;

/// The lines a `range` selected: their text, the first and the last line
/// number, and each requested range as `{startLine, endLine}`.
pub(crate) struct LineWindow {
    pub(crate) text: String,
    pub(crate) start_line: NonZeroUsize,
    pub(crate) end_line: usize,
    pub(crate) ranges: Vec<Value>,
}

/// The window `range` selects from `text`, or `None` when the caller asked
/// for no range and the whole text is answered.
pub(crate) fn selected_lines(text: &str, range: Option<String>) -> Result<Option<LineWindow>, String> {
    match range {
        Some(range) => line_window(text, &range).map(Some),
        None => Ok(None),
    }
}

fn line_number(text: &str, full_range: &str) -> Result<NonZeroUsize, String> {
    text.trim().parse::<NonZeroUsize>().map_err(|error| {
        format!("invalid range {full_range}: {text:?} is not a line number counted from the first line ({error})")
    })
}

/// One comma-separated part of a range: `N`, `N-M`, `N-` (to the end) or
/// `N+K` (K lines from N). The end is cut to the text's last line.
fn parse_line_range_token(
    token: &str,
    line_count: usize,
    full_range: &str,
) -> Result<(NonZeroUsize, usize), String> {
    let trimmed = token.trim();
    let (start, end) = if let Some((first, count)) = trimmed.split_once('+') {
        let start = line_number(first, full_range)?;
        let count = line_number(count, full_range)?;
        let end = start
            .get()
            .saturating_add(count.get() - usize::from(STEP));
        (start, end)
    } else if let Some((first, last)) = trimmed.split_once('-') {
        let start = line_number(first, full_range)?;
        let end = if last.is_empty() {
            line_count
        } else {
            line_number(last, full_range)?.get()
        };
        (start, end)
    } else {
        let line = line_number(trimmed, full_range)?;
        (line, line.get())
    };
    if end < start.get() {
        return Err(format!(
            "invalid range {full_range}: it ends at line {end}, before its start at line {start}"
        ));
    }
    Ok((start, end.min(line_count)))
}

fn line_window(text: &str, range: &str) -> Result<LineWindow, String> {
    let lines: Vec<&str> = text.lines().collect();
    let mut chunks = Vec::new();
    let mut ranges = Vec::new();
    let mut bounds = Vec::new();
    for token in range.split(',') {
        let (start, end) = parse_line_range_token(token, lines.len(), range)?;
        let start_index = (start.get() - usize::from(STEP)).min(lines.len());
        if start_index < end {
            chunks.push(lines[start_index..end].join("\n"));
        }
        ranges.push(json!({"startLine": start, "endLine": end}));
        bounds.push((start, end));
    }
    let start_line = bounds.iter().map(|(start, _)| *start).min();
    let end_line = bounds.iter().map(|(_, end)| *end).max();
    match (start_line, end_line) {
        (Some(start_line), Some(end_line)) => Ok(LineWindow {
            text: chunks.join("\n"),
            start_line,
            end_line,
            ranges,
        }),
        _ => Err(format!("invalid range {range:?}: it names no lines")),
    }
}

/// The lines of `content` and whether it ends with a line break.
pub(crate) fn split_edit_lines(content: &str) -> (Vec<String>, bool) {
    let (body, has_trailing) = match content.strip_suffix('\n') {
        Some(body) => (body, true),
        None => (content, false),
    };
    (body.lines().map(ToString::to_string).collect(), has_trailing)
}

/// The whole change as one unified-diff hunk from the first line: every old
/// line removed, every new line added. Nothing when the text is unchanged.
pub(crate) fn simple_diff(path: &str, before: &str, after: &str) -> String {
    if before == after {
        return String::new();
    }
    let (old_lines, _) = split_edit_lines(before);
    let (new_lines, _) = split_edit_lines(after);
    let mut out = vec![
        format!("--- {path}"),
        format!("+++ {path}"),
        format!(
            "@@ -{FIRST},{} +{FIRST},{} @@",
            old_lines.len(),
            new_lines.len()
        ),
    ];
    for line in &old_lines {
        out.push(format!("-{line}"));
    }
    for line in &new_lines {
        out.push(format!("+{line}"));
    }
    out.join("\n")
}

/// A content hash's first sixteen bits in upper-case hex: short enough to
/// copy into an edit, long enough that a changed file shows a different tag.
pub(crate) fn snapshot_tag(hash: &str) -> String {
    let width = hex::encode(u16::MAX.to_be_bytes()).len();
    hash.chars()
        .take(width)
        .collect::<String>()
        .to_ascii_uppercase()
}

pub(crate) fn snapshot_name(path: &str, hash: &str) -> String {
    format!("{}#{}", path, snapshot_tag(hash))
}
