use serde_json::{json, Value};
use std::fs;
use std::path::Path;
use std::sync::LazyLock;
use std::time::Duration;

use crate::tool_runtime::shared::{
    jail_path, line_window, mime_type_for_path, sha256_hex, string_input, u64_input, MAX_READ_BYTES,
};
use crate::tool_runtime::ToolRuntime;

fn readable_text_from_html(raw: &str) -> String {
    let mut text = raw.to_string();
    for pattern in [
        r"(?is)<script\b[^>]*>.*?</script>",
        r"(?is)<style\b[^>]*>.*?</style>",
        r"(?is)<[^>]+>",
    ] {
        if let Ok(re) = regex::Regex::new(pattern) {
            text = re.replace_all(&text, " ").to_string();
        }
    }
    text.replace("&nbsp;", " ")
        .replace("&amp;", "&")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&#39;", "'")
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

fn readable_text_from_json(raw: &str) -> Result<String, String> {
    let value: Value = serde_json::from_str(raw).map_err(|e| e.to_string())?;
    serde_json::to_string_pretty(&value).map_err(|e| e.to_string())
}

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

fn readable_text_from_delimited(raw: &str, delimiter: char) -> String {
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

fn tag_text(xml: &str, tag: &str) -> String {
    let pattern = format!(
        r"(?is)<{}\b[^>]*>(.*?)</{}>",
        regex::escape(tag),
        regex::escape(tag)
    );
    regex::Regex::new(&pattern)
        .ok()
        .and_then(|re| re.captures(xml))
        .and_then(|cap| cap.get(1).map(|m| readable_text_from_html(m.as_str())))
        .unwrap_or_default()
}

/// An entry's `<link href="...">`, which the entry loop asks for once per item.
static LINK_HREF: LazyLock<regex::Regex> = LazyLock::new(|| {
    regex::Regex::new(r#"(?is)<link[^>]*href=["']([^"']+)["'][^>]*>"#)
        .expect("static feed link pattern")
});

fn readable_text_from_feed(raw: &str) -> String {
    let mut lines = Vec::new();
    let feed_title = tag_text(raw, "title");
    if !feed_title.is_empty() {
        lines.push(format!("# {feed_title}"));
    }
    if let Ok(item_re) = regex::Regex::new(r"(?is)<(item|entry)\b[^>]*>(.*?)</(item|entry)>") {
        for cap in item_re.captures_iter(raw) {
            let body = cap.get(2).map(|m| m.as_str()).unwrap_or("");
            let title = {
                let t = tag_text(body, "title");
                if t.is_empty() {
                    "(untitled)".into()
                } else {
                    t
                }
            };
            let link = LINK_HREF
                .captures(body)
                .and_then(|c| c.get(1).map(|m| readable_text_from_html(m.as_str())))
                .unwrap_or_else(|| tag_text(body, "link"));
            if link.is_empty() {
                lines.push(format!("- {title}"));
            } else {
                lines.push(format!("- {title} — {link}"));
            }
        }
    }
    if lines.is_empty() {
        readable_text_from_html(raw)
    } else {
        lines.join("\n")
    }
}

fn decode_pdf_string(raw: &str) -> String {
    let mut out = String::new();
    let mut chars = raw.chars().peekable();
    while let Some(ch) = chars.next() {
        if ch != '\\' {
            out.push(ch);
            continue;
        }
        match chars.next() {
            Some('n') => out.push('\n'),
            Some('r') => out.push('\r'),
            Some('t') => out.push('\t'),
            Some('b') => out.push('\u{0008}'),
            Some('f') => out.push('\u{000c}'),
            Some('(') => out.push('('),
            Some(')') => out.push(')'),
            Some('\\') => out.push('\\'),
            Some(other) => out.push(other),
            None => break,
        }
    }
    out
}

fn readable_text_from_pdf(bytes: &[u8]) -> String {
    let raw = String::from_utf8_lossy(bytes);
    let mut out = Vec::new();
    if let Ok(single) = regex::Regex::new(r"\((?:\\.|[^\\)])*\)\s*Tj") {
        for mat in single.find_iter(&raw) {
            let text = mat.as_str();
            if let Some(end) = text.rfind(')') {
                out.push(decode_pdf_string(&text[1..end]));
            }
        }
    }
    if let Ok(array) = regex::Regex::new(r"\[((?:\s*\((?:\\.|[^\\)])*\)\s*[-0-9.]*\s*)+)\]\s*TJ") {
        let string_re = regex::Regex::new(r"\((?:\\.|[^\\)])*\)").unwrap();
        for cap in array.captures_iter(&raw) {
            let parts = string_re
                .find_iter(cap.get(1).map(|m| m.as_str()).unwrap_or(""))
                .filter_map(|m| {
                    let s = m.as_str();
                    s.rfind(')').map(|end| decode_pdf_string(&s[1..end]))
                })
                .collect::<Vec<_>>();
            if !parts.is_empty() {
                out.push(parts.join(""));
            }
        }
    }
    let mut seen = std::collections::HashSet::new();
    out.into_iter()
        .filter(|line| seen.insert(line.clone()))
        .collect::<Vec<_>>()
        .join("\n")
        .trim()
        .to_string()
}

fn readable_text_from_notebook(raw: &str) -> Result<String, String> {
    let notebook: Value = serde_json::from_str(raw).map_err(|e| e.to_string())?;
    let cells = notebook
        .get("cells")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    let mut out = Vec::new();
    for (idx, cell) in cells.iter().enumerate() {
        let kind = cell
            .get("cell_type")
            .and_then(Value::as_str)
            .unwrap_or("cell");
        let source = match cell.get("source") {
            Some(Value::Array(parts)) => parts
                .iter()
                .map(|part| part.as_str().unwrap_or(&part.to_string()).to_string())
                .collect::<Vec<_>>()
                .join(""),
            Some(Value::String(text)) => text.clone(),
            _ => String::new(),
        };
        out.push(
            format!("# %% [{kind}] cell:{}\n{}", idx + 1, source)
                .trim()
                .to_string(),
        );
    }
    Ok(out.join("\n\n"))
}

pub(super) fn readable_text_for_document(
    bytes: &[u8],
    path: &Path,
    content_type: Option<&str>,
) -> Result<String, String> {
    let ext = path
        .extension()
        .and_then(|ext| ext.to_str())
        .unwrap_or("")
        .to_ascii_lowercase();
    let type_hint = content_type.unwrap_or("").to_ascii_lowercase();
    if ext == "pdf" || type_hint.contains("pdf") {
        return Ok(readable_text_from_pdf(bytes));
    }
    let raw = String::from_utf8_lossy(bytes).to_string();
    if ext == "ipynb" {
        return readable_text_from_notebook(&raw);
    }
    if ext == "json" || type_hint.contains("json") {
        return readable_text_from_json(&raw);
    }
    if ext == "csv" || type_hint.contains("csv") {
        return Ok(readable_text_from_delimited(&raw, ','));
    }
    if ext == "tsv"
        || ext == "tab"
        || type_hint.contains("tsv")
        || type_hint.contains("tab-separated-values")
    {
        return Ok(readable_text_from_delimited(&raw, '\t'));
    }
    if ext == "html" || ext == "htm" || type_hint.contains("html") {
        return Ok(readable_text_from_html(&raw));
    }
    if ext == "xml"
        || ext == "rss"
        || ext == "atom"
        || type_hint.contains("xml")
        || type_hint.contains("rss")
        || type_hint.contains("atom")
    {
        return Ok(readable_text_from_feed(&raw));
    }
    Ok(raw)
}

pub(crate) fn read_document(runtime: &ToolRuntime<'_>, input: &Value) -> Result<Value, String> {
    let path = string_input(input, "path").ok_or("read_document requires path")?;
    let max_bytes =
        u64_input(input, "maxBytes", MAX_READ_BYTES).clamp(1_000, MAX_READ_BYTES) as usize;
    let file = jail_path(runtime.cwd, &path)?;
    let bytes = fs::read(&file).map_err(|e| e.to_string())?;
    let readable = readable_text_for_document(&bytes, &file, None)?;
    let selected = if let Some(range) = string_input(input, "range") {
        line_window(&readable, &range)?
    } else {
        (readable.clone(), 0, 0, Vec::new())
    };
    let output_bytes = selected.0.as_bytes();
    let slice = &output_bytes[..output_bytes.len().min(max_bytes)];
    Ok(
        json!({"path": path, "bytes": readable.len(), "truncated": output_bytes.len() > slice.len(), "mimeType": mime_type_for_path(&file), "text": String::from_utf8_lossy(slice), "startLine": if selected.3.is_empty() { Value::Null } else { json!(selected.1) }, "endLine": if selected.3.is_empty() { Value::Null } else { json!(selected.2) }, "ranges": if selected.3.is_empty() { Value::Null } else { json!(selected.3) }, "sha256": sha256_hex(&bytes)}),
    )
}

pub(crate) fn fetch_readable_url(
    _runtime: &ToolRuntime<'_>,
    input: &Value,
) -> Result<Value, String> {
    let url = string_input(input, "url").ok_or("fetch_readable_url requires url")?;
    if !url.starts_with("http://") && !url.starts_with("https://") {
        return Err("fetch_readable_url requires http(s) URL".into());
    }
    let max_bytes = u64_input(input, "maxBytes", 200_000).clamp(1_000, 1_000_000) as usize;
    let timeout_ms = u64_input(input, "timeoutMs", 30_000).clamp(1_000, 120_000);
    let client = reqwest::blocking::Client::builder()
        .timeout(Duration::from_millis(timeout_ms))
        .build()
        .map_err(|e| e.to_string())?;
    let response = client.get(&url).send().map_err(|e| e.to_string())?;
    let status = response.status().as_u16();
    let ok = (200..300).contains(&status);
    let content_type = response
        .headers()
        .get(reqwest::header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .map(ToString::to_string);
    let bytes = response.bytes().map_err(|e| e.to_string())?;
    let url_path = url::Url::parse(&url)
        .ok()
        .map(|url| url.path().to_string())
        .unwrap_or_default();
    let readable =
        readable_text_for_document(&bytes, Path::new(&url_path), content_type.as_deref())?;
    let selected = if let Some(range) = string_input(input, "range") {
        line_window(&readable, &range)?
    } else {
        (readable.clone(), 0, 0, Vec::new())
    };
    let output_bytes = selected.0.as_bytes();
    let slice = &output_bytes[..output_bytes.len().min(max_bytes)];
    Ok(
        json!({"url": url, "status": status, "ok": ok, "contentType": content_type, "bytes": readable.len(), "sha256": sha256_hex(&bytes), "truncated": output_bytes.len() > slice.len(), "text": String::from_utf8_lossy(slice), "startLine": if selected.3.is_empty() { Value::Null } else { json!(selected.1) }, "endLine": if selected.3.is_empty() { Value::Null } else { json!(selected.2) }, "ranges": if selected.3.is_empty() { Value::Null } else { json!(selected.3) }}),
    )
}
