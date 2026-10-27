//! Turning one recorded session into something a person can read or keep.
//!
//! Split out of `slash/session/mod.rs`, which had grown past the module line
//! cap.

use serde_json::Value;

/// The id, directory and events `read_session_value` writes into every
/// exported session. A value without them was not read through it and is
/// refused naming the field, never shown under the name "session" or an
/// empty path.
fn session_parts(session: &Value) -> Result<(&str, &str, &[Value]), String> {
    let field = |name: &str| {
        session
            .get(name)
            .and_then(Value::as_str)
            .ok_or_else(|| format!("the session to export records no {name}"))
    };
    let events = session
        .get("events")
        .and_then(Value::as_array)
        .ok_or("the session to export records no events list")?;
    Ok((field("id")?, field("path")?, events))
}

/// An event's heading: its time and its type, both of which every recorded
/// event carries; one without either is refused naming the event.
fn event_label(event: &Value) -> Result<String, String> {
    let field = |name: &str| {
        event
            .get(name)
            .and_then(Value::as_str)
            .ok_or_else(|| format!("a recorded event carries no {name}: {event}"))
    };
    Ok(format!("{} {}", field("ts")?, field("type")?))
}

pub(crate) fn slash_session_text(session: &Value) -> Result<String, String> {
    let (id, path, events) = session_parts(session)?;
    let mut out = vec![format!("Session: {id}"), format!("Path: {path}"), String::new()];
    for event in events {
        out.push(format!("## {}", event_label(event)?));
        out.push(format!("{:#}", event["data"]));
        out.push(String::new());
    }
    Ok(out.join("\n"))
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
    let (id, path, events) = session_parts(session)?;
    if format == "markdown" {
        let mut out = format!("# Jeden session {id}\n\n{path}\n\n");
        for event in events {
            let label = event_label(event)?;
            let data = format!("{:#}", event["data"]);
            out.push_str(&format!("## {}\n\n```json\n{}\n```\n\n", label, data));
        }
        return Ok(out);
    }
    if format == "html" {
        let id = slash_html_escape(id);
        let path = slash_html_escape(path);
        let mut body = String::new();
        for event in events {
            let label = slash_html_escape(&event_label(event)?);
            let data = slash_html_escape(&format!("{:#}", event["data"]));
            body.push_str(&format!(
                "<section><h2>{}</h2><pre>{}</pre></section>\n",
                label, data
            ));
        }
        return Ok(format!("<!doctype html><html><head><meta charset=\"utf-8\"><title>Jeden session {}</title></head><body><h1>Jeden session {}</h1><p>{}</p>{}</body></html>\n", id, id, path, body));
    }
    Err(format!("unsupported session export format: {}", format))
}
