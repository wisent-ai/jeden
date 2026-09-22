//! Turning one recorded session into something a person can read or keep.
//!
//! Split out of `slash/session/mod.rs`, which had grown past the module line
//! cap.

use serde_json::Value;

pub(crate) fn slash_session_text(session: &Value) -> String {
    let mut out = vec![
        format!(
            "Session: {}",
            session
                .get("id")
                .and_then(Value::as_str)
                .unwrap_or("session")
        ),
        format!(
            "Path: {}",
            session.get("path").and_then(Value::as_str).unwrap_or("")
        ),
        String::new(),
    ];
    for event in session
        .get("events")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default()
    {
        out.push(
            format!(
                "## {} {}",
                event.get("ts").and_then(Value::as_str).unwrap_or(""),
                event.get("type").and_then(Value::as_str).unwrap_or("")
            )
            .trim()
            .to_string(),
        );
        out.push(
            serde_json::to_string_pretty(event.get("data").unwrap_or(&Value::Null))
                .unwrap_or_else(|_| "{}".into()),
        );
        out.push(String::new());
    }
    out.join("\n")
}

fn slash_html_escape(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

pub(crate) fn slash_session_export(session: &Value, format: &str) -> Result<String, String> {
    if format == "json" {
        return Ok(serde_json::to_string_pretty(session).map_err(|e| e.to_string())? + "\n");
    }
    if format == "markdown" || format == "md" {
        let mut out = format!(
            "# Jeden session {}\n\n{}\n\n",
            session
                .get("id")
                .and_then(Value::as_str)
                .unwrap_or("session"),
            session.get("path").and_then(Value::as_str).unwrap_or("")
        );
        for event in session
            .get("events")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default()
        {
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
        let id = slash_html_escape(
            session
                .get("id")
                .and_then(Value::as_str)
                .unwrap_or("session"),
        );
        let path = slash_html_escape(session.get("path").and_then(Value::as_str).unwrap_or(""));
        let mut body = String::new();
        for event in session
            .get("events")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default()
        {
            let label = slash_html_escape(
                format!(
                    "{} {}",
                    event.get("ts").and_then(Value::as_str).unwrap_or(""),
                    event.get("type").and_then(Value::as_str).unwrap_or("")
                )
                .trim(),
            );
            let data = slash_html_escape(
                &serde_json::to_string_pretty(event.get("data").unwrap_or(&Value::Null))
                    .unwrap_or_else(|_| "{}".into()),
            );
            body.push_str(&format!(
                "<section><h2>{}</h2><pre>{}</pre></section>\n",
                label, data
            ));
        }
        return Ok(format!("<!doctype html><html><head><meta charset=\"utf-8\"><title>Jeden session {}</title></head><body><h1>Jeden session {}</h1><p>{}</p>{}</body></html>\n", id, id, path, body));
    }
    Err(format!("unsupported session export format: {}", format))
}
