use serde_json::Value;

use crate::tool_runtime::shared::split_edit_lines;

fn inserted_lines(value: Option<&Value>) -> Result<Vec<String>, String> {
    match value {
        None | Some(Value::Null) => Ok(Vec::new()),
        Some(Value::Array(items)) => items
            .iter()
            .map(|item| {
                item.as_str()
                    .map(ToString::to_string)
                    .ok_or_else(|| "content lines must be strings".to_string())
            })
            .collect(),
        Some(Value::String(text)) => {
            if text.is_empty() {
                Ok(Vec::new())
            } else {
                let body = text.strip_suffix('\n').unwrap_or(text);
                Ok(if body.is_empty() {
                    Vec::new()
                } else {
                    body.lines().map(ToString::to_string).collect()
                })
            }
        }
        _ => Err("content must be a string or array".into()),
    }
}

/// What one line edit does, spelled as the `edit_file` op names it. The
/// visual `edit` tool resolves its hunks to these same values, so an op is
/// declared here once and never respelled as a word.
#[derive(Clone, Copy, Debug, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum LineEditKind {
    Replace,
    Delete,
    InsertBefore,
    InsertAfter,
}

impl LineEditKind {
    const ALL: &'static [LineEditKind] = &[
        LineEditKind::Replace,
        LineEditKind::Delete,
        LineEditKind::InsertBefore,
        LineEditKind::InsertAfter,
    ];

    /// Every op's name, for the tool schema and refusals.
    pub(crate) fn names() -> Vec<String> {
        Self::ALL
            .iter()
            .map(|kind| crate::vocabulary::spelling(*kind))
            .collect()
    }

    /// Whether this edit spans existing lines (and so must not overlap
    /// another), rather than adding lines at one position.
    fn spans_lines(self) -> bool {
        matches!(self, LineEditKind::Replace | LineEditKind::Delete)
    }
}

/// One line edit in the file's original 1-based coordinates: a range edit
/// covers `start..=end`, an insert has one position, `start`.
#[derive(Clone, Debug)]
pub(super) struct LineEdit {
    pub(super) kind: LineEditKind,
    pub(super) start: usize,
    pub(super) end: usize,
    pub(super) content: Vec<String>,
}

impl LineEdit {
    /// An insert of `content` before or after `line`.
    pub(super) fn insert(kind: LineEditKind, line: usize, content: Vec<String>) -> Self {
        Self {
            kind,
            start: line,
            end: line,
            content,
        }
    }
}

fn parsed_line_edits(ops: &Value) -> Result<Vec<LineEdit>, String> {
    let Some(items) = ops.as_array() else {
        return Err("ops are required".into());
    };
    let mut parsed = Vec::new();
    for (index, op) in items.iter().enumerate() {
        let Some(obj) = op.as_object() else {
            return Err("op must be an object".into());
        };
        let named = obj
            .get("op")
            .ok_or_else(|| format!("edit op {index} names no op"))?;
        let kind: LineEditKind = serde_json::from_value(named.clone()).map_err(|_| {
            format!(
                "unknown edit op: {named}; the ops are {}",
                LineEditKind::names().join(", ")
            )
        })?;
        let start = obj
            .get("start")
            .or_else(|| obj.get("startLine"))
            .or_else(|| obj.get("line"))
            .and_then(Value::as_u64)
            .ok_or("start must be a 1-based line number")? as usize;
        // A range edit states the last line it covers; an insert has one
        // position, its start.
        let end = if kind.spans_lines() {
            obj.get("end")
                .or_else(|| obj.get("endLine"))
                .and_then(Value::as_u64)
                .ok_or_else(|| {
                    format!("edit op {named} needs end (or endLine): the last line it covers")
                })? as usize
        } else {
            start
        };
        parsed.push(LineEdit {
            kind,
            start,
            end,
            content: inserted_lines(obj.get("content"))?,
        });
    }
    Ok(parsed)
}

/// Each edit checked against the file it applies to, then every range
/// against every other: no edit reaches past the file and no two ranges
/// overlap.
fn checked(lines: &[String], edits: &[LineEdit]) -> Result<(), String> {
    if edits.is_empty() {
        return Err("ops are required".into());
    }
    for edit in edits {
        if edit.start < 1 {
            return Err("start must be a 1-based line number".into());
        }
        if edit.end < edit.start {
            return Err("end must be >= start".into());
        }
        if edit.kind.spans_lines() && edit.end > lines.len() {
            return Err("edit range is past end of file".into());
        }
        if !edit.kind.spans_lines() && edit.start > lines.len() + 1 {
            return Err("insert line is past end of file".into());
        }
    }
    let mut ranges: Vec<_> = edits
        .iter()
        .filter(|edit| edit.kind.spans_lines())
        .collect();
    ranges.sort_by_key(|edit| edit.start);
    for pair in ranges.windows(2) {
        if pair[1].start <= pair[0].end {
            return Err("edit ranges overlap".into());
        }
    }
    Ok(())
}

/// `content` with `edits` applied, every edit in original coordinates.
pub(super) fn apply_line_edits(content: &str, edits: &[LineEdit]) -> Result<String, String> {
    let (mut lines, trailing) = split_edit_lines(content);
    checked(&lines, edits)?;
    let mut ordered: Vec<(usize, &LineEdit)> = edits.iter().enumerate().collect();
    ordered.sort_by(|(a_index, a), (b_index, b)| {
        b.start.cmp(&a.start).then_with(|| b_index.cmp(a_index))
    });
    for (_, edit) in ordered {
        let content = edit.content.clone();
        match edit.kind {
            LineEditKind::Replace => {
                lines.splice(edit.start - 1..edit.end, content);
            }
            LineEditKind::Delete => {
                lines.drain(edit.start - 1..edit.end);
            }
            LineEditKind::InsertBefore => {
                lines.splice(edit.start - 1..edit.start - 1, content);
            }
            LineEditKind::InsertAfter => {
                let at = edit.start.min(lines.len());
                lines.splice(at..at, content);
            }
        }
    }
    let next = lines.join("\n");
    Ok(if trailing { format!("{next}\n") } else { next })
}

/// The `edit_file` tool's `ops` read into line edits and applied.
pub(super) fn apply_line_edit_ops(content: &str, ops: &Value) -> Result<String, String> {
    apply_line_edits(content, &parsed_line_edits(ops)?)
}
