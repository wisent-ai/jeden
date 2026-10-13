//! Finding sessions: `jeden sessions` lists the session directories and
//! `jeden sessions search` names the newest event of each session whose
//! payload holds a query.

use serde_json::{json, Value};
use std::fs;

use super::read_session_value;
use crate::session_root;

/// `jeden sessions [limit] [--json]`: the session directories, one per line
/// or as a JSON array. A session root that exists but cannot be read is that
/// error, not "No sessions found".
pub(crate) fn list_sessions(limit: Option<usize>, json: bool) -> Result<String, String> {
    let root = session_root();
    let mut rows = vec![];
    match fs::read_dir(&root) {
        Ok(entries) => {
            for (listed, entry) in entries.enumerate() {
                if limit.is_some_and(|limit| listed >= limit) {
                    break;
                }
                let entry = entry.map_err(|error| {
                    format!(
                        "the session root {} cannot be read: {error}",
                        root.display()
                    )
                })?;
                rows.push(entry.file_name().to_string_lossy().to_string());
            }
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => {
            return Err(format!(
                "the session root {} cannot be read: {error}",
                root.display()
            ))
        }
    }
    if json {
        return serde_json::to_string_pretty(&rows)
            .map(|text| text + "\n")
            .map_err(|error| error.to_string());
    }
    Ok(if rows.is_empty() {
        "No sessions found.\n".into()
    } else {
        rows.join("\n") + "\n"
    })
}

/// `jeden sessions search QUERY [LIMIT] [--json]`: the newest event of each
/// session whose payload holds QUERY, case-insensitively. `positionals` are
/// the words after `search`. A store that cannot be read, or an event that
/// lacks the fields every recorded event carries, is refused naming it.
pub(crate) fn search_sessions_command(
    positionals: &[String],
    json: bool,
) -> Result<String, String> {
    use crate::cli::invocation::refusal::usage;
    if let [_, _, unexpected, ..] = positionals {
        return Err(usage(format!(
            "sessions search: unexpected argument {unexpected:?}; usage: jeden sessions search QUERY [LIMIT] [--json]"
        )));
    }
    let query = positionals
        .first()
        .ok_or_else(|| usage("sessions search requires a query"))?
        .trim()
        .to_ascii_lowercase();
    if query.is_empty() {
        return Err(usage("sessions search requires a non-empty query"));
    }
    let limit = match positionals.get(1) {
        None => None,
        Some(raw) => Some(raw.parse::<usize>().map_err(|_| {
            usage(format!(
                "sessions search takes a whole number of sessions to scan after the query, not {raw:?}"
            ))
        })?),
    };
    let root = session_root();
    let mut entries = match fs::read_dir(&root) {
        Ok(entries) => entries
            .collect::<Result<Vec<_>, _>>()
            .map_err(|error| format!("cannot list sessions in {}: {error}", root.display()))?
            .into_iter()
            .map(|entry| entry.path())
            .collect::<Vec<_>>(),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Vec::new(),
        Err(error) => {
            return Err(format!(
                "cannot list sessions in {}: {error}",
                root.display()
            ))
        }
    };
    entries.sort_by(|a, b| b.file_name().cmp(&a.file_name()));
    if let Some(limit) = limit {
        entries.truncate(limit);
    }
    let mut hits = Vec::new();
    for dir in entries {
        let session = read_session_value(&dir.display().to_string())
            .map_err(|error| format!("cannot search session {}: {}", dir.display(), error))?;
        let field = |value: &Value, name: &str| {
            value
                .get(name)
                .and_then(Value::as_str)
                .map(str::to_string)
                .ok_or_else(|| format!("session {} has a record without its {name}", dir.display()))
        };
        let id = field(&session, "id")?;
        let events = session
            .get("events")
            .and_then(Value::as_array)
            .ok_or_else(|| format!("session {id} has no events list"))?;
        for event in events {
            // An event that carries no payload has nothing to match.
            let Some(data) = event.get("data") else {
                continue;
            };
            let text = serde_json::to_string(data).map_err(|error| {
                format!("session {id}: an event payload cannot be read: {error}")
            })?;
            if !text.to_ascii_lowercase().contains(&query) {
                continue;
            }
            hits.push(json!({
                "session": id,
                "ts": field(event, "ts")?,
                "type": field(event, "type")?,
                // Whitespace-collapsed full event text.
                "snippet": text.split_whitespace().collect::<Vec<_>>().join(" "),
            }));
            break;
        }
    }
    if json {
        return serde_json::to_string_pretty(&hits)
            .map(|text| text + "\n")
            .map_err(|error| error.to_string());
    }
    // Each field of a hit, or the field named as missing: a dash read as a
    // session or time called "-".
    let field = |hit: &Value, name: &str| match hit[name].as_str() {
        Some(text) => text.to_string(),
        None => format!("(no {name})"),
    };
    Ok(hits
        .iter()
        .map(|hit| {
            format!(
                "{}\t{}\t{}\t{}\n",
                field(hit, "session"),
                field(hit, "ts"),
                field(hit, "type"),
                field(hit, "snippet"),
            )
        })
        .collect())
}
