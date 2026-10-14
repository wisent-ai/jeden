//! Writing a recorded session out in the form somebody asked for, and listing
//! what it produced.
//!
//! Split out of `cli/reports/sessions.rs`, which had grown past the module
//! line cap.

use super::{read_session_value, session_dir_for};
use crate::Args;
use serde_json::Value;
use std::fs;

fn html_escape(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

pub(crate) fn render_session_export(session: &Value, format: &str) -> Result<String, String> {
    if format == "json" {
        return Ok(serde_json::to_string_pretty(session).map_err(|e| e.to_string())? + "\n");
    }
    let id = session
        .get("id")
        .and_then(Value::as_str)
        .unwrap_or("session");
    let path = session.get("path").and_then(Value::as_str).unwrap_or("");
    let events = session
        .get("events")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    if format == "markdown" {
        let mut out = format!("# Jeden session {}\n\n{}\n\n", id, path);
        for event in events {
            let label = format!(
                "{} {}",
                event.get("ts").and_then(Value::as_str).unwrap_or(""),
                event.get("type").and_then(Value::as_str).unwrap_or("")
            )
            .trim()
            .to_string();
            let data = format!("{:#}", event["data"]);
            out.push_str(&format!("## {}\n\n```json\n{}\n```\n\n", label, data));
        }
        return Ok(out);
    }
    if format == "html" {
        let mut sections = String::new();
        for event in events {
            let label = html_escape(
                format!(
                    "{} {}",
                    event.get("ts").and_then(Value::as_str).unwrap_or(""),
                    event.get("type").and_then(Value::as_str).unwrap_or("")
                )
                .trim(),
            );
            let body = html_escape(&format!("{:#}", event["data"]));
            sections.push_str(&format!(
                "<section class=\"event\"><h2>{}</h2><pre>{}</pre></section>\n",
                label, body
            ));
        }
        return Ok(format!("<!doctype html>\n<html><head><meta charset=\"utf-8\"><title>Jeden session {}</title><style>body{{font-family:ui-sans-serif,system-ui,sans-serif;margin:2rem;background:#fafafa;color:#111}}.event{{border:1px solid #ddd;border-radius:8px;background:white;margin:1rem 0;padding:1rem}}pre{{white-space:pre-wrap;overflow-wrap:anywhere}}</style></head><body><h1>Jeden session {}</h1><p>{}</p>{}</body></html>\n", html_escape(id), html_escape(id), html_escape(path), sections));
    }
    Err(format!("unsupported session export format: {}", format))
}

/// `jeden export <session> <output> [--html|--markdown]`: write the session
/// to a file. Printing a session is `jeden show`; an export without an
/// output printed the same session a second way, so one is required.
pub(crate) fn export_session_command(args: &Args) -> Result<String, String> {
    const USAGE: &str =
        "usage: jeden export <session-id-or-path> <output> [--html|--markdown]; `jeden show` prints a session";
    let (id, rest) = args.positionals.split_first().ok_or_else(|| {
        crate::cli::invocation::refusal::usage(format!(
            "export requires a session id or path: {USAGE}"
        ))
    })?;
    let mut format = "json".to_string();
    let mut output = None;
    for arg in rest {
        if arg == "--html" {
            format = "html".into();
        } else if arg == "--markdown" {
            format = "markdown".into();
        } else if arg.starts_with("--") || output.is_some() {
            // An unknown flag is not a file name, and a second path is not
            // silently preferred over the first.
            return Err(crate::cli::invocation::refusal::usage(format!(
                "export: unexpected argument {arg:?}; {USAGE}"
            )));
        } else {
            output = Some(arg.clone());
        }
    }
    let Some(path) = output else {
        return Err(crate::cli::invocation::refusal::usage(format!(
            "export requires the file to write: {USAGE}"
        )));
    };
    let payload = render_session_export(&read_session_value(id)?, &format)?;
    fs::write(&path, &payload)
        .map_err(|error| format!("cannot write the export to {path}: {error}"))?;
    Ok(format!("{}\n", path))
}

/// `jeden artifacts <session> [--json]`: each artifact's name and size in
/// bytes, one per line or as a JSON array.
pub(crate) fn list_artifacts_command(id_or_path: &str, json: bool) -> Result<String, String> {
    let dir = session_dir_for(id_or_path).join("artifacts");
    let mut rows: Vec<(String, u64)> = vec![];
    match fs::read_dir(&dir) {
        Ok(entries) => {
            for entry in entries {
                let entry = entry.map_err(|error| {
                    format!(
                        "read artifact directory entry at {}: {error}",
                        dir.display()
                    )
                })?;
                let meta = entry.metadata().map_err(|error| {
                    format!(
                        "read artifact metadata at {}: {error}",
                        entry.path().display()
                    )
                })?;
                if meta.is_file() {
                    rows.push((entry.file_name().to_string_lossy().to_string(), meta.len()));
                }
            }
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => {
            return Err(format!(
                "the artifacts of {id_or_path} at {} cannot be read: {error}",
                dir.display()
            ))
        }
    }
    rows.sort();
    if json {
        let listed: Vec<Value> = rows
            .iter()
            .map(|(name, bytes)| serde_json::json!({ "name": name, "bytes": bytes }))
            .collect();
        return serde_json::to_string_pretty(&listed)
            .map(|text| text + "\n")
            .map_err(|error| error.to_string());
    }
    Ok(rows
        .iter()
        .map(|(name, bytes)| format!("{name}\t{bytes}\n"))
        .collect())
}

pub(crate) fn artifact_command(args: &Args) -> Result<String, String> {
    let mut it = args.positionals.iter();
    let id = it.next().ok_or_else(|| {
        crate::cli::invocation::refusal::usage("artifact requires a session id or path")
    })?;
    let name = it.next().ok_or_else(|| {
        crate::cli::invocation::refusal::usage("artifact requires an artifact name")
    })?;
    let output = it.next();
    if let Some(argument) = it.next() {
        return Err(crate::cli::invocation::refusal::usage(format!(
            "artifact: unexpected argument {argument:?}; usage: jeden artifact <session-id-or-path> <name> [output]"
        )));
    }
    let root = session_dir_for(id).join("artifacts");
    let file = root.join(name);
    let canonical_root = fs::canonicalize(&root)
        .map_err(|error| format!("cannot resolve artifact root {}: {error}", root.display()))?;
    let canonical_file = fs::canonicalize(&file)
        .map_err(|error| format!("cannot resolve artifact {}: {error}", file.display()))?;
    if !canonical_file.starts_with(&canonical_root) {
        return Err(format!("artifact path escapes session: {}", name));
    }
    let content = fs::read_to_string(&canonical_file).map_err(|error| {
        format!(
            "cannot read artifact {} as UTF-8: {error}",
            canonical_file.display()
        )
    })?;
    if let Some(output) = output {
        fs::write(output, &content)
            .map_err(|error| format!("cannot write artifact to {output}: {error}"))?;
        Ok(format!("{}\n", output))
    } else {
        Ok(if content.ends_with('\n') {
            content
        } else {
            content + "\n"
        })
    }
}
