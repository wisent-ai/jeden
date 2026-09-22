//! Turning each document format this product understands into readable text.
//!
//! Split out of `tool_runtime/read/document.rs`, which had grown past the
//! module line cap.

use serde_json::Value;
use std::sync::LazyLock;

pub(super) fn readable_text_from_html(raw: &str) -> String {
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

pub(super) fn readable_text_from_json(raw: &str) -> Result<String, String> {
    let value: Value = serde_json::from_str(raw).map_err(|e| e.to_string())?;
    serde_json::to_string_pretty(&value).map_err(|e| e.to_string())
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

pub(super) fn readable_text_from_feed(raw: &str) -> String {
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

pub(super) fn readable_text_from_pdf(bytes: &[u8]) -> String {
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

pub(super) fn readable_text_from_notebook(raw: &str) -> Result<String, String> {
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
