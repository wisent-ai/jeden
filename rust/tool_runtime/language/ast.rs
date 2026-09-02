use serde_json::{json, Value};
use std::fs;
use std::path::{Path, PathBuf};
use std::time::Duration;
use tree_sitter::{Language, Parser, Query, QueryCursor, StreamingIterator};

use crate::cli::sessions::{
    claim_pending_action, complete_pending_action, create_pending_action, discard_pending_action,
    PendingActionCreate,
};
use crate::tool_runtime::shared::{jail_path, sha256_hex, simple_diff, string_input, u64_input};
use crate::tool_runtime::ToolRuntime;

const MAX_AST_BYTES: u64 = 4 * 1024 * 1024;
const MAX_MATCHES: usize = 1_000;
const PENDING_TTL: Duration = Duration::from_secs(600);

fn language(name: &str, path: &Path) -> Result<Language, String> {
    let inferred = path
        .extension()
        .and_then(|value| value.to_str())
        .unwrap_or("");
    match if name.is_empty() { inferred } else { name } {
        "rs" | "rust" => Ok(tree_sitter_rust::LANGUAGE.into()),
        "js" | "jsx" | "javascript" => Ok(tree_sitter_javascript::LANGUAGE.into()),
        "ts" | "typescript" => Ok(tree_sitter_typescript::LANGUAGE_TYPESCRIPT.into()),
        "tsx" => Ok(tree_sitter_typescript::LANGUAGE_TSX.into()),
        "py" | "python" => Ok(tree_sitter_python::LANGUAGE.into()),
        other => Err(format!("unsupported AST language: {other}")),
    }
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
    if !metadata.is_file() || metadata.len() > MAX_AST_BYTES {
        return Err(format!(
            "AST input must be a file no larger than {MAX_AST_BYTES} bytes"
        ));
    }
    let bytes = fs::read(&path).map_err(|error| error.to_string())?;
    std::str::from_utf8(&bytes).map_err(|_| "AST input is not UTF-8".to_string())?;
    let language = language(&string_input(input, "language").unwrap_or_default(), &path)?;
    Ok((label, path, bytes, language))
}

fn selected_capture(query: &Query, input: &Value) -> Result<u32, String> {
    if let Some(name) = string_input(input, "capture") {
        query
            .capture_index_for_name(&name)
            .ok_or_else(|| format!("query has no @{name} capture"))
    } else if query.capture_names().is_empty() {
        Err("AST query must contain at least one capture".into())
    } else {
        Ok(0)
    }
}

fn ranges(
    source: &[u8],
    tree: &tree_sitter::Tree,
    query: &Query,
    capture: u32,
    limit: usize,
) -> Vec<(usize, usize, Value)> {
    let mut cursor = QueryCursor::new();
    let mut stream = cursor.matches(query, tree.root_node(), source);
    let mut out = Vec::new();
    while let Some(item) = stream.next() {
        for found in item.captures.iter().filter(|found| found.index == capture) {
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
                    "start": {"line": start.row + 1, "column": start.column + 1},
                    "end": {"line": end.row + 1, "column": end.column + 1},
                    "text": text,
                }),
            ));
            if out.len() >= limit {
                return out;
            }
        }
    }
    out
}

pub(crate) fn ast_search(runtime: &ToolRuntime<'_>, input: &Value) -> Result<Value, String> {
    if runtime.operation.cancellation().is_cancelled() {
        return Err("AST search cancelled".into());
    }
    let query_source = string_input(input, "query").ok_or("ast_search requires query")?;
    let limit = u64_input(input, "limit", 100).clamp(1, MAX_MATCHES as u64) as usize;
    let (label, _, bytes, language) = source(runtime, input)?;
    let tree = parse(&bytes, &language)?;
    let query = Query::new(&language, &query_source)
        .map_err(|error| format!("invalid AST query: {error}"))?;
    let capture = selected_capture(&query, input)?;
    let matches = ranges(&bytes, &tree, &query, capture, limit)
        .into_iter()
        .map(|(_, _, value)| value)
        .collect::<Vec<_>>();
    Ok(
        json!({"ok": true, "path": label, "query": query_source, "matches": matches, "truncated": matches.len() == limit}),
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
    let path = jail_path(runtime.cwd, &claim.target)?;
    let current = fs::read(&path).map_err(|error| error.to_string())?;
    let current_sha = sha256_hex(&current);
    if current_sha != claim.expected_sha256 {
        return Err(format!(
            "AST rewrite conflict for {}: expected {}, found {}",
            claim.target, claim.expected_sha256, current_sha
        ));
    }
    let parent = path.parent().ok_or("AST rewrite target has no parent")?;
    let temp = parent.join(format!(".jeden-{}.tmp", claim.id));
    fs::write(&temp, &claim.payload).map_err(|error| error.to_string())?;
    fs::rename(&temp, &path).map_err(|error| {
        let _ = fs::remove_file(&temp);
        error.to_string()
    })?;
    complete_pending_action(artifact_dir, &claim.id)?;
    Ok(
        json!({"ok":true,"pendingId":claim.id,"applied":true,"path":claim.target,"sha256":sha256_hex(&claim.payload)}),
    )
}

pub(crate) fn ast_rewrite(runtime: &ToolRuntime<'_>, input: &Value) -> Result<Value, String> {
    match string_input(input, "action")
        .unwrap_or_else(|| "preview".into())
        .as_str()
    {
        "apply" => return resolve_pending(runtime, input, true),
        "discard" => return resolve_pending(runtime, input, false),
        "preview" => {}
        other => return Err(format!("unsupported ast_rewrite action: {other}")),
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
    let mut selected = ranges(&bytes, &tree, &query, capture, MAX_MATCHES);
    selected.sort_by_key(|(start, end, _)| (*start, *end));
    for pair in selected.windows(2) {
        if pair[0].1 > pair[1].0 {
            return Err("AST rewrite captures overlap".into());
        }
    }
    let mut rewritten = bytes.clone();
    for (start, end, _) in selected.iter().rev() {
        let original = String::from_utf8_lossy(&bytes[*start..*end]);
        let text = replacement.replace("$TEXT", &original);
        rewritten.splice(*start..*end, text.bytes());
    }
    let before = std::str::from_utf8(&bytes).map_err(|_| "AST input is not UTF-8")?;
    let after =
        std::str::from_utf8(&rewritten).map_err(|_| "AST replacement produced invalid UTF-8")?;
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
            ttl_seconds: PENDING_TTL.as_secs(),
        },
    )?;
    Ok(
        json!({"ok": true, "preview": true, "pendingId": id, "expiresInMs": PENDING_TTL.as_millis(), "path": label, "expectedSha256": expected_sha256, "matchCount": selected.len(), "diff": diff}),
    )
}
