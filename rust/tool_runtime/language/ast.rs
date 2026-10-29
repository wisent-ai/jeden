use serde_json::{json, Value};
use std::fs;
use std::path::{Path, PathBuf};
use tree_sitter::{Language, Parser, Query, QueryCursor, StreamingIterator};

use crate::cli::sessions::{
    claim_pending_action, complete_pending_action, create_pending_action, discard_pending_action,
    PendingActionCreate,
};
use crate::tool_runtime::shared::{
    count_input, jail_path, jail_write_path, sha256_hex, simple_diff, string_input,
};
use crate::tool_runtime::ToolRuntime;

/// The grammars the AST tools parse, named by language or file extension.
#[derive(serde::Deserialize)]
#[serde(rename_all = "lowercase")]
enum Grammar {
    #[serde(alias = "rs")]
    Rust,
    #[serde(alias = "js", alias = "jsx")]
    Javascript,
    #[serde(alias = "ts")]
    Typescript,
    Tsx,
    #[serde(alias = "py")]
    Python,
}

/// The grammar the caller names, or the one the file's extension names when
/// the caller names none; refused when neither names a supported grammar.
fn language(name: Option<String>, path: &Path) -> Result<Language, String> {
    let named = match name.filter(|name| !name.is_empty()) {
        Some(name) => name,
        None => path
            .extension()
            .and_then(|value| value.to_str())
            .map(str::to_owned)
            .ok_or_else(|| {
                format!(
                    "{} has no extension that names its language; state language",
                    path.display()
                )
            })?,
    };
    let grammar = serde_json::from_value::<Grammar>(Value::String(named.clone()))
        .map_err(|error| format!("unsupported AST language {named}: {error}"))?;
    Ok(match grammar {
        Grammar::Rust => tree_sitter_rust::LANGUAGE.into(),
        Grammar::Javascript => tree_sitter_javascript::LANGUAGE.into(),
        Grammar::Typescript => tree_sitter_typescript::LANGUAGE_TYPESCRIPT.into(),
        Grammar::Tsx => tree_sitter_typescript::LANGUAGE_TSX.into(),
        Grammar::Python => tree_sitter_python::LANGUAGE.into(),
    })
}

fn parse(source: &[u8], language: &Language) -> Result<tree_sitter::Tree, String> {
    let mut parser = Parser::new();
    parser
        .set_language(language)
        .map_err(|error| error.to_string())?;
    parser
        .parse(source, None)
        .ok_or_else(|| "AST parser returned no tree".to_string())
}

fn source(
    runtime: &ToolRuntime<'_>,
    input: &Value,
) -> Result<(String, PathBuf, Vec<u8>, Language), String> {
    let label = string_input(input, "path").ok_or("AST tool requires path")?;
    let path = jail_path(runtime.cwd, &label)?;
    let metadata = fs::metadata(&path).map_err(|error| error.to_string())?;
    if !metadata.is_file() {
        return Err(format!("AST input must be a file: {label}"));
    }
    let bytes = fs::read(&path).map_err(|error| error.to_string())?;
    std::str::from_utf8(&bytes).map_err(|error| format!("AST input is not UTF-8: {error}"))?;
    let language = language(string_input(input, "language"), &path)?;
    Ok((label, path, bytes, language))
}

/// The capture the caller names, or the query's first capture.
fn selected_capture(query: &Query, input: &Value) -> Result<u32, String> {
    let name = match string_input(input, "capture") {
        Some(name) => name,
        None => query
            .capture_names()
            .first()
            .map(|name| name.to_string())
            .ok_or("AST query must contain at least one capture")?,
    };
    query
        .capture_index_for_name(&name)
        .ok_or_else(|| format!("query has no @{name} capture"))
}

fn ranges(
    source: &[u8],
    tree: &tree_sitter::Tree,
    query: &Query,
    capture: u32,
    limit: Option<usize>,
) -> Result<(Vec<(usize, usize, Value)>, bool), String> {
    let mut cursor = QueryCursor::new();
    let mut stream = cursor.matches(query, tree.root_node(), source);
    let mut out = Vec::new();
    let mut truncated = false;
    'matches: while let Some(item) = stream.next() {
        for found in item.captures.iter().filter(|found| found.index == capture) {
            if limit.is_some_and(|limit| out.len() == limit) {
                truncated = true;
                break 'matches;
            }
            let node = found.node;
            let start = node.start_position();
            let end = node.end_position();
            let text = String::from_utf8_lossy(&source[node.byte_range()]);
            out.push((
                node.start_byte(),
                node.end_byte(),
                json!({
                    "kind": node.kind(),
                    "startByte": node.start_byte(),
                    "endByte": node.end_byte(),
                    // Tree-sitter counts rows and columns from zero; people count from one.
                    "start": {"line": start.row + usize::from(crate::tally::STEP), "column": start.column + usize::from(crate::tally::STEP)},
                    "end": {"line": end.row + usize::from(crate::tally::STEP), "column": end.column + usize::from(crate::tally::STEP)},
                    "text": text,
                }),
            ));
        }
    }
    drop(stream);
    if cursor.did_exceed_match_limit() {
        return Err(format!(
            "AST query exceeded the parser's in-progress match capacity ({}); refusing incomplete captures",
            cursor.match_limit()
        ));
    }
    Ok((out, truncated))
}

pub(crate) fn ast_search(runtime: &ToolRuntime<'_>, input: &Value) -> Result<Value, String> {
    if runtime.operation.cancellation().is_cancelled() {
        return Err("AST search cancelled".into());
    }
    let query_source = string_input(input, "query").ok_or("ast_search requires query")?;
    let limit = count_input(input, "limit", "ast_search")? as usize;
    let (label, _, bytes, language) = source(runtime, input)?;
    let tree = parse(&bytes, &language)?;
    let query = Query::new(&language, &query_source)
        .map_err(|error| format!("invalid AST query: {error}"))?;
    let capture = selected_capture(&query, input)?;
    let (selected, truncated) = ranges(&bytes, &tree, &query, capture, Some(limit))?;
    let matches = selected
        .into_iter()
        .map(|(_, _, value)| value)
        .collect::<Vec<_>>();
    Ok(
        json!({"ok": true, "path": label, "query": query_source, "matches": matches, "truncated": truncated}),
    )
}

fn resolve_pending(runtime: &ToolRuntime<'_>, input: &Value, apply: bool) -> Result<Value, String> {
    let id = string_input(input, "pendingId").ok_or("pending action requires pendingId")?;
    let artifact_dir = runtime
        .artifact_dir
        .ok_or("AST pending actions require a recorded session")?;
    if !apply {
        discard_pending_action(artifact_dir, &runtime.operation, &id)?;
        return Ok(json!({"ok":true,"pendingId":id,"discarded":true}));
    }
    if !runtime.allow_write {
        return Err("applying AST rewrite requires --allow-write".into());
    }
    let claim = claim_pending_action(artifact_dir, &runtime.operation, &id)?;
    if claim.kind != "ast_rewrite" {
        return Err(format!(
            "pending action {} has kind {}, expected ast_rewrite",
            claim.id, claim.kind
        ));
    }
    let path = jail_write_path(runtime.cwd, &claim.target)?;
    let current = fs::read(&path).map_err(|error| {
        format!(
            "AST rewrite target {} cannot be read: {error}",
            claim.target
        )
    })?;
    let current_sha = sha256_hex(&current);
    if current_sha != claim.expected_sha256 {
        return Err(format!(
            "AST rewrite conflict for {}: expected {}, found {}",
            claim.target, claim.expected_sha256, current_sha
        ));
    }
    // The rewritten source keeps the file's mode: a temp written beside it
    // and renamed over it left an executable script without its x bits.
    crate::tool_runtime::runtime_ops::durable::replace_keeping_mode(&path, &claim.payload)?;
    complete_pending_action(artifact_dir, &claim.id)?;
    Ok(
        json!({"ok":true,"pendingId":claim.id,"applied":true,"path":claim.target,"sha256":sha256_hex(&claim.payload)}),
    )
}

pub(crate) fn ast_rewrite(runtime: &ToolRuntime<'_>, input: &Value) -> Result<Value, String> {
    // Without an action the rewrite is previewed.
    match string_input(input, "action").as_deref() {
        Some("apply") => return resolve_pending(runtime, input, true),
        Some("discard") => return resolve_pending(runtime, input, false),
        None | Some("preview") => {}
        Some(other) => return Err(format!("unsupported ast_rewrite action: {other}")),
    }
    if runtime.operation.cancellation().is_cancelled() {
        return Err("AST rewrite cancelled".into());
    }
    let query_source = string_input(input, "query").ok_or("ast_rewrite preview requires query")?;
    let replacement =
        string_input(input, "replacement").ok_or("ast_rewrite preview requires replacement")?;
    let (label, _path, bytes, language) = source(runtime, input)?;
    let tree = parse(&bytes, &language)?;
    let query = Query::new(&language, &query_source)
        .map_err(|error| format!("invalid AST query: {error}"))?;
    let capture = selected_capture(&query, input)?;
    // A rewrite covers every match: rewriting the first N and reporting the
    // file rewritten would leave the rest unchanged without a word.
    let (mut selected, _) = ranges(&bytes, &tree, &query, capture, None)?;
    selected.sort_by_key(|(start, end, _)| (*start, *end));
    for (earlier, later) in selected
        .iter()
        .zip(selected.iter().skip(usize::from(crate::tally::STEP)))
    {
        if earlier.1 > later.0 {
            return Err("AST rewrite captures overlap".into());
        }
    }
    let mut rewritten = bytes.clone();
    for (start, end, _) in selected.iter().rev() {
        let original = String::from_utf8_lossy(&bytes[*start..*end]);
        let text = replacement.replace("$TEXT", &original);
        rewritten.splice(*start..*end, text.bytes());
    }
    let before =
        std::str::from_utf8(&bytes).map_err(|error| format!("AST input is not UTF-8: {error}"))?;
    let after = std::str::from_utf8(&rewritten)
        .map_err(|error| format!("AST replacement produced invalid UTF-8: {error}"))?;
    let diff = simple_diff(&label, before, after);
    let expected_sha256 = sha256_hex(&bytes);
    let artifact_dir = runtime
        .artifact_dir
        .ok_or("AST pending actions require a recorded session")?;
    let id = create_pending_action(
        artifact_dir,
        &runtime.operation,
        PendingActionCreate {
            kind: "ast_rewrite".into(),
            target: label.clone(),
            expected_sha256: expected_sha256.clone(),
            payload: rewritten,
            preview: diff.clone(),
        },
    )?;
    Ok(
        json!({"ok": true, "preview": true, "pendingId": id, "path": label, "expectedSha256": expected_sha256, "matchCount": selected.len(), "diff": diff}),
    )
}
