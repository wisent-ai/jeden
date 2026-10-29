//! Turning each document format this product understands into readable text.
//!
//! Split out of `tool_runtime/read/document.rs`, which had grown past the
//! module line cap.

use crate::tally::FIRST;
use regex::Regex;
use serde_json::Value;
use std::sync::LazyLock;

/// Compiles one of this module's own patterns; a pattern that does not
/// compile is a defect in this file, not in the document being read.
fn pattern(source: &str) -> Regex {
    Regex::new(source).expect("a document pattern compiled into Jeden")
}

static SCRIPT: LazyLock<Regex> = LazyLock::new(|| pattern(r"(?is)<script\b[^>]*>.*?</script>"));
static STYLE: LazyLock<Regex> = LazyLock::new(|| pattern(r"(?is)<style\b[^>]*>.*?</style>"));
static TAG: LazyLock<Regex> = LazyLock::new(|| pattern(r"(?is)<[^>]+>"));
static PDF_SHOWN: LazyLock<Regex> = LazyLock::new(|| pattern(r"\((?<text>(?:\\.|[^\\)])*)\)\s*Tj"));
static PDF_SHOWN_ARRAY: LazyLock<Regex> =
    LazyLock::new(|| pattern(r"\[(?<items>(?:\s*\((?:\\.|[^\\)])*\)\s*[-\d.]*\s*)+)\]\s*TJ"));
static PDF_STRING: LazyLock<Regex> = LazyLock::new(|| pattern(r"\((?<text>(?:\\.|[^\\)])*)\)"));

pub(super) fn readable_text_from_html(raw: &str) -> String {
    let text = SCRIPT.replace_all(raw, " ");
    let text = STYLE.replace_all(&text, " ");
    let text = TAG.replace_all(&text, " ");
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
    let value: Value =
        serde_json::from_str(raw).map_err(|error| format!("the document is not JSON: {error}"))?;
    serde_json::to_string_pretty(&value)
        .map_err(|error| format!("the JSON document cannot be laid out: {error}"))
}

/// Markup read as a feed when it is one — its title, then each entry's title
/// and first link — and as a page's text when the feed parser finds no feed
/// in it.
pub(super) fn readable_text_from_feed(raw: &str) -> String {
    let feed = match feed_rs::parser::parse(raw.as_bytes()) {
        Ok(feed) => feed,
        Err(_not_a_feed) => return readable_text_from_html(raw),
    };
    let mut lines = Vec::new();
    if let Some(title) = &feed.title {
        lines.push(format!("# {}", readable_text_from_html(&title.content)));
    }
    for entry in &feed.entries {
        let title = entry
            .title
            .as_ref()
            .map(|title| readable_text_from_html(&title.content));
        let link = entry.links.first().map(|link| link.href.as_str());
        lines.push(match (title, link) {
            (Some(title), Some(link)) => format!("- {title} — {link}"),
            (Some(title), None) => format!("- {title}"),
            (None, Some(link)) => format!("- (an entry with no title) — {link}"),
            (None, None) => "- (an entry with neither title nor link)".to_owned(),
        });
    }
    lines.join("\n")
}

/// A PDF literal string's text with its escapes resolved. Backspace and form
/// feed are layout, not text: a form feed reads as a line break, a backspace
/// as nothing.
fn decode_pdf_string(raw: &str) -> String {
    let mut out = String::new();
    let mut chars = raw.chars();
    while let Some(ch) = chars.next() {
        if ch != '\\' {
            out.push(ch);
            continue;
        }
        match chars.next() {
            Some('n') | Some('f') => out.push('\n'),
            Some('r') => out.push('\r'),
            Some('t') => out.push('\t'),
            Some('b') => {}
            Some(other) => out.push(other),
            None => break,
        }
    }
    out
}

pub(super) fn readable_text_from_pdf(bytes: &[u8]) -> String {
    let raw = String::from_utf8_lossy(bytes);
    let mut out = Vec::new();
    for shown in PDF_SHOWN.captures_iter(&raw) {
        if let Some(text) = shown.name("text") {
            out.push(decode_pdf_string(text.as_str()));
        }
    }
    for shown in PDF_SHOWN_ARRAY.captures_iter(&raw) {
        let Some(items) = shown.name("items") else {
            continue;
        };
        let parts = PDF_STRING
            .captures_iter(items.as_str())
            .filter_map(|part| part.name("text"))
            .map(|text| decode_pdf_string(text.as_str()))
            .collect::<Vec<_>>();
        if !parts.is_empty() {
            out.push(parts.join(""));
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

/// A notebook as `# %% [kind] cell:N` sections, the cells numbered from the
/// first. A cell without its type or source, or with a source line that is not
/// text, is refused naming the cell: the notebook format requires both.
pub(super) fn readable_text_from_notebook(raw: &str) -> Result<String, String> {
    let notebook: Value =
        serde_json::from_str(raw).map_err(|error| format!("the notebook is not JSON: {error}"))?;
    let cells = notebook
        .get("cells")
        .and_then(Value::as_array)
        .ok_or("the notebook holds no cells list")?;
    let mut out = Vec::new();
    for (number, cell) in (FIRST..).zip(cells) {
        let kind = cell
            .get("cell_type")
            .and_then(Value::as_str)
            .ok_or_else(|| format!("notebook cell {number} has no cell_type"))?;
        let source = match cell.get("source") {
            Some(Value::Array(parts)) => parts
                .iter()
                .map(|part| {
                    part.as_str().ok_or_else(|| {
                        format!("notebook cell {number} has a source line that is not text: {part}")
                    })
                })
                .collect::<Result<Vec<_>, _>>()?
                .join(""),
            Some(Value::String(text)) => text.clone(),
            Some(other) => {
                return Err(format!(
                    "notebook cell {number} has a source that is not text: {other}"
                ))
            }
            None => return Err(format!("notebook cell {number} has no source")),
        };
        out.push(
            format!("# %% [{kind}] cell:{number}\n{source}")
                .trim()
                .to_string(),
        );
    }
    Ok(out.join("\n\n"))
}
