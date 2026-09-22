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
    if format == "markdown" || format == "md" {
        let mut out = format!("# Jeden session {}\n\n{}\n\n", id, path);
        for event in events {
            let label = format!(
                "{} {}",
                event.get("ts").and_then(Value::as_str).unwrap_or(""),
                event.get("type").and_then(Value::as_str).unwrap_or("")
            )
            .trim()
            .to_string();
            let data = serde_json::to_string_pretty(event.get("data").unwrap_or(&Value::Null))
                .unwrap_or_else(|_| "{}".into());
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
            let body = html_escape(
                &serde_json::to_string_pretty(event.get("data").unwrap_or(&Value::Null))
                    .unwrap_or_else(|_| "{}".into()),
            );
            sections.push_str(&format!(
                "<section class=\"event\"><h2>{}</h2><pre>{}</pre></section>\n",
                label, body
            ));
        }
        return Ok(format!("<!doctype html>\n<html><head><meta charset=\"utf-8\"><title>Jeden session {}</title><style>body{{font-family:ui-sans-serif,system-ui,sans-serif;margin:2rem;background:#fafafa;color:#111}}.event{{border:1px solid #ddd;border-radius:8px;background:white;margin:1rem 0;padding:1rem}}pre{{white-space:pre-wrap;overflow-wrap:anywhere}}</style></head><body><h1>Jeden session {}</h1><p>{}</p>{}</body></html>\n", html_escape(id), html_escape(id), html_escape(path), sections));
    }
    Err(format!("unsupported session export format: {}", format))
}

pub(crate) fn export_session_command(args: &Args) -> Result<String, String> {
    let (id, rest) = args
        .positionals
        .split_first()
        .ok_or("export requires a session id or path")?;
    let mut format = "json".to_string();
    let mut output = None;
    for arg in rest {
        if arg == "--html" {
            format = "html".into();
        } else if arg == "--markdown" {
            format = "markdown".into();
        } else {
            output = Some(arg.clone());
        }
    }
    let payload = render_session_export(&read_session_value(id)?, &format)?;
    if let Some(path) = output {
        fs::write(&path, &payload).map_err(|e| e.to_string())?;
        Ok(format!("{}\n", path))
    } else {
        Ok(payload)
    }
}

pub(crate) fn list_artifacts_command(id_or_path: &str) -> Result<String, String> {
    let dir = session_dir_for(id_or_path).join("artifacts");
    let mut rows = vec![];
    if let Ok(entries) = fs::read_dir(dir) {
        for entry in entries.flatten() {
            if let Ok(meta) = entry.metadata() {
                if meta.is_file() {
                    rows.push(format!(
                        "{}\t{}",
                        entry.file_name().to_string_lossy(),
                        meta.len()
                    ));
                }
            }
        }
    }
    rows.sort();
    Ok(if rows.is_empty() {
        String::new()
    } else {
        rows.join("\n") + "\n"
    })
}

pub(crate) fn artifact_command(args: &Args) -> Result<String, String> {
    let mut it = args.positionals.iter();
    let id = it.next().ok_or("artifact requires a session id or path")?;
    let name = it.next().ok_or("artifact requires an artifact name")?;
    let output = it.next();
    let root = session_dir_for(id).join("artifacts");
    let file = root.join(name);
    let canonical_root = fs::canonicalize(&root).map_err(|e| e.to_string())?;
    let canonical_file = fs::canonicalize(&file).map_err(|e| e.to_string())?;
    if !canonical_file.starts_with(&canonical_root) {
        return Err(format!("artifact path escapes session: {}", name));
    }
    let content = fs::read_to_string(&canonical_file).map_err(|e| e.to_string())?;
    if let Some(output) = output {
        fs::write(output, &content).map_err(|e| e.to_string())?;
        Ok(format!("{}\n", output))
    } else {
        Ok(if content.ends_with('\n') {
            content
        } else {
            content + "\n"
        })
    }
}
