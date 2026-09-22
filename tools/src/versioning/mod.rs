//! The version gate's side: the fleet's versioning rule, the published
//! baseline it compares against, and the version this repository commits.

mod baseline;
mod rule;

use regex::Regex;
use serde_json::{json, Value};
use std::fs;
use std::path::Path;

const USAGE: &str = "usage: jeden-tools versioning <decide --current VERSION --published-surface FILE --candidate-surface FILE [--breaking] [--json] | baseline [--marker | --stdout] | manifest-version [CARGO_TOML]>";

pub(crate) fn run(arguments: &[String]) -> Result<u8, String> {
    let Some((action, rest)) = arguments.split_first() else {
        return Err(USAGE.into());
    };
    match (action.as_str(), rest) {
        ("decide", options) => decide(options),
        ("baseline", []) => baseline::regenerate(false).map(|()| 0),
        ("baseline", [flag]) if flag == "--stdout" => baseline::regenerate(true).map(|()| 0),
        ("baseline", [flag]) if flag == "--marker" => {
            println!("{}", baseline::marker()?);
            Ok(0)
        }
        ("manifest-version", []) => manifest_version(&crate::repository_root().join("Cargo.toml")),
        ("manifest-version", [path]) => manifest_version(Path::new(path)),
        _ => Err(USAGE.into()),
    }
}

fn surface_file(path: &str) -> Result<Vec<String>, String> {
    let text = fs::read_to_string(path).map_err(|error| format!("{path}: {error}"))?;
    let document: Value =
        serde_json::from_str(&text).map_err(|error| format!("{path}: {error}"))?;
    let names = document["surface"].as_array().ok_or_else(|| {
        format!(
            "{path}: no \"surface\" key. A surface document is {{\"surface\": [\"name\", ...]}}"
        )
    })?;
    names
        .iter()
        .map(|name| name.as_str().map(str::to_string))
        .collect::<Option<Vec<_>>>()
        .ok_or_else(|| format!("{path}: a surface name is not a string"))
}

fn decide(options: &[String]) -> Result<u8, String> {
    let mut current = None;
    let mut published = None;
    let mut candidate = None;
    let mut breaking = false;
    let mut as_json = false;
    let mut remaining = options.iter();
    while let Some(option) = remaining.next() {
        let mut value = || {
            remaining
                .next()
                .cloned()
                .ok_or(format!("{option} needs a value"))
        };
        match option.as_str() {
            "--current" => current = Some(value()?),
            "--published-surface" => published = Some(value()?),
            "--candidate-surface" => candidate = Some(value()?),
            "--breaking" => breaking = true,
            "--json" => as_json = true,
            other => return Err(format!("unknown option {other}; {USAGE}")),
        }
    }
    let (Some(current), Some(published), Some(candidate)) = (current, published, candidate) else {
        return Err(USAGE.into());
    };
    let answer = rule::decide(
        &current,
        &surface_file(&published)?,
        &surface_file(&candidate)?,
        breaking,
    )
    .map_err(|refusal| refusal.to_string())?;
    let document = json!({
        "current": answer.current,
        "change": answer.change.name(),
        "next": answer.next,
        "removed": answer.removed,
        "added": answer.added,
    });
    if as_json {
        let text = serde_json::to_string_pretty(&document).map_err(|error| error.to_string())?;
        println!("{text}");
    } else {
        println!("current: {}", answer.current);
        println!("change: {}", answer.change.name());
        println!("next: {}", answer.next);
        for (key, names) in [("removed", &answer.removed), ("added", &answer.added)] {
            if !names.is_empty() {
                println!("{key}: {}", names.join(", "));
            }
        }
    }
    Ok(0)
}

/// `[package].version` as Cargo.toml commits it.
fn manifest_version(path: &Path) -> Result<u8, String> {
    let text = fs::read_to_string(path).map_err(|error| format!("{}: {error}", path.display()))?;
    let assignment =
        Regex::new(r#"^version\s*=\s*"([^"]+)"\s*$"#).map_err(|error| error.to_string())?;
    let mut section = String::new();
    for line in text.lines().map(str::trim) {
        if line.starts_with('[') {
            section = line.to_string();
        } else if section == "[package]" {
            if let Some(captures) = assignment.captures(line) {
                println!("{}", &captures[1]);
                return Ok(0);
            }
        }
    }
    Err(format!(
        "{} has no non-empty [package].version",
        path.display()
    ))
}
