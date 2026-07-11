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

#[derive(Clone)]
struct LineEditOp {
    kind: String,
    start: usize,
    end: usize,
    content: Vec<String>,
    index: usize,
}

fn normalize_line_edit_ops(content: &str, ops: &Value) -> Result<Vec<LineEditOp>, String> {
    let (lines, _) = split_edit_lines(content);
    let Some(items) = ops.as_array() else {
        return Err("ops are required".into());
    };
    if items.is_empty() {
        return Err("ops are required".into());
    }
    let mut normalized = Vec::new();
    for (index, op) in items.iter().enumerate() {
        let Some(obj) = op.as_object() else {
            return Err("op must be an object".into());
        };
        let kind = obj
            .get("op")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string();
        if !matches!(
            kind.as_str(),
            "replace" | "delete" | "insert_before" | "insert_after"
        ) {
            return Err(format!("unknown edit op: {kind}"));
        }
        let start = obj
            .get("start")
            .or_else(|| obj.get("startLine"))
            .or_else(|| obj.get("line"))
            .and_then(Value::as_u64)
            .ok_or("start must be a 1-based line number")? as usize;
        let end = obj
            .get("end")
            .or_else(|| obj.get("endLine"))
            .and_then(Value::as_u64)
            .map(|n| n as usize)
            .unwrap_or(start);
        if start < 1 {
            return Err("start must be a 1-based line number".into());
        }
        if end < start {
            return Err("end must be >= start".into());
        }
        if matches!(kind.as_str(), "replace" | "delete") && end > lines.len() {
            return Err("edit range is past end of file".into());
        }
        if matches!(kind.as_str(), "insert_before" | "insert_after") && start > lines.len() + 1 {
            return Err("insert line is past end of file".into());
        }
        normalized.push(LineEditOp {
            kind,
            start,
            end,
            content: inserted_lines(obj.get("content"))?,
            index,
        });
    }
    let mut ranges: Vec<_> = normalized
        .iter()
        .filter(|op| matches!(op.kind.as_str(), "replace" | "delete"))
        .collect();
    ranges.sort_by_key(|op| op.start);
    for pair in ranges.windows(2) {
        if pair[1].start <= pair[0].end {
            return Err("edit ranges overlap".into());
        }
    }
    Ok(normalized)
}

pub(super) fn apply_line_edit_ops(content: &str, ops: &Value) -> Result<String, String> {
    let (mut lines, trailing) = split_edit_lines(content);
    let mut normalized = normalize_line_edit_ops(content, ops)?;
    normalized.sort_by(|a, b| b.start.cmp(&a.start).then_with(|| b.index.cmp(&a.index)));
    for op in normalized {
        match op.kind.as_str() {
            "replace" => {
                lines.splice(op.start - 1..op.end, op.content);
            }
            "delete" => {
                lines.drain(op.start - 1..op.end);
            }
            "insert_before" => {
                lines.splice(
                    op.start.saturating_sub(1)..op.start.saturating_sub(1),
                    op.content,
                );
            }
            "insert_after" => {
                let at = op.start.min(lines.len());
                lines.splice(at..at, op.content);
            }
            _ => unreachable!(),
        }
    }
    let next = lines.join("\n");
    Ok(if trailing { format!("{next}\n") } else { next })
}
