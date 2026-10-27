//! Reading a table out of a delimited file the way a spreadsheet would: the
//! fields as the `csv` crate reads them (quoted fields, doubled quotes, CRLF
//! line ends, rows of differing width), laid out as a Markdown table whose
//! first row is its header.
//!
//! Split out of `tool_runtime/read/document.rs`, which had grown past the
//! module line cap.

fn markdown_cell(value: &str) -> String {
    value
        .replace('|', "\\|")
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

fn markdown_row(cells: &[String]) -> String {
    format!("| {} |", cells.join(" | "))
}

/// The table in `raw`, separated by `delimiter`, as Markdown; a file whose
/// rows are all blank is no table and reads as no text. A delimiter wider
/// than one byte, or a record the reader cannot read, is refused naming why.
pub(super) fn readable_text_from_delimited(raw: &str, delimiter: char) -> Result<String, String> {
    let delimiter = u8::try_from(u32::from(delimiter))
        .map_err(|error| format!("the table delimiter {delimiter:?} is not one byte: {error}"))?;
    let mut reader = csv::ReaderBuilder::new()
        .delimiter(delimiter)
        .has_headers(false)
        .flexible(true)
        .from_reader(raw.as_bytes());
    let mut rows = Vec::new();
    for record in reader.records() {
        let record = record.map_err(|error| format!("the table cannot be read: {error}"))?;
        // A row of blank cells carries nothing.
        if record.iter().any(|cell| !cell.trim().is_empty()) {
            rows.push(record.iter().map(markdown_cell).collect::<Vec<_>>());
        }
    }
    let Some(columns) = rows.iter().map(Vec::len).max() else {
        return Ok(String::new());
    };
    // Shorter rows are padded with empty cells to the widest one.
    rows.iter_mut().for_each(|row| row.resize(columns, String::new()));
    let mut lines = Vec::new();
    if let Some((header, body)) = rows.split_first() {
        lines.push(markdown_row(header));
        lines.push(markdown_row(&vec!["---".to_string(); columns]));
        lines.extend(body.iter().map(|row| markdown_row(row)));
    }
    Ok(lines.join("\n"))
}
