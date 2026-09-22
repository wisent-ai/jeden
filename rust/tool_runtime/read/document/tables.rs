//! Reading a table out of a delimited file the way a spreadsheet would.
//!
//! Split out of `tool_runtime/read/document.rs`, which had grown past the
//! module line cap.

fn parse_delimited_rows(raw: &str, delimiter: char) -> Vec<Vec<String>> {
    let mut rows = Vec::new();
    let mut row = Vec::new();
    let mut field = String::new();
    let mut quoted = false;
    let chars: Vec<char> = raw.chars().collect();
    let mut i = 0;
    while i < chars.len() {
        let ch = chars[i];
        if quoted {
            if ch == '"' {
                if chars.get(i + 1) == Some(&'"') {
                    field.push('"');
                    i += 1;
                } else {
                    quoted = false;
                }
            } else {
                field.push(ch);
            }
        } else if ch == '"' {
            quoted = true;
        } else if ch == delimiter {
            row.push(std::mem::take(&mut field));
        } else if ch == '\n' || ch == '\r' {
            if ch == '\r' && chars.get(i + 1) == Some(&'\n') {
                i += 1;
            }
            row.push(std::mem::take(&mut field));
            rows.push(std::mem::take(&mut row));
        } else {
            field.push(ch);
        }
        i += 1;
    }
    if !field.is_empty() || !row.is_empty() {
        row.push(field);
        rows.push(row);
    }
    rows
}

fn markdown_cell(value: &str) -> String {
    value
        .replace('|', "\\|")
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

pub(super) fn readable_text_from_delimited(raw: &str, delimiter: char) -> String {
    let rows: Vec<Vec<String>> = parse_delimited_rows(raw, delimiter)
        .into_iter()
        .filter(|row| row.iter().any(|cell| !cell.trim().is_empty()))
        .collect();
    if rows.is_empty() {
        return String::new();
    }
    let column_count = rows.iter().map(Vec::len).max().unwrap_or(0);
    let normalized = rows
        .iter()
        .take(51)
        .map(|row| {
            (0..column_count)
                .map(|idx| markdown_cell(row.get(idx).map(String::as_str).unwrap_or("")))
                .collect::<Vec<_>>()
        })
        .collect::<Vec<_>>();
    let mut lines = Vec::new();
    lines.push(format!("| {} |", normalized[0].join(" | ")));
    lines.push(format!("| {} |", vec!["---"; column_count].join(" | ")));
    for row in normalized.iter().skip(1) {
        lines.push(format!("| {} |", row.join(" | ")));
    }
    if rows.len() > 51 {
        lines.push(format!(
            "\n[truncated after 50 data rows; total rows: {}]",
            rows.len()
        ));
    }
    lines.join("\n")
}
