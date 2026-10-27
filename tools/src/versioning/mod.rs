//! The version gate's side: the fleet's versioning rule (the autoversion
//! crate, github.com/lbartoszcze/AutoVersion, held to its docs/FIXTURES.md),
//! the published baseline it compares against, and the version this
//! repository commits.

mod baseline;

use regex::Regex;
use serde_json::Value;
use std::fs;
use std::path::Path;

const USAGE: &str = "usage: jeden-tools versioning <decide --current VERSION --published-surface FILE --candidate-surface FILE [--breaking] [--json] | baseline [--marker | --stdout] | manifest-version [CARGO_TOML]>";

/// The exit status of a command that succeeded, as POSIX exit() defines it:
/// https://pubs.opengroup.org/onlinepubs/9699919799/functions/exit.html
const SUCCEEDED: u8 = 0;

pub(crate) fn run(arguments: &[String]) -> Result<u8, String> {
    let Some((action, rest)) = arguments.split_first() else {
        return Err(USAGE.into());
    };
    match (action.as_str(), rest) {
        ("decide", options) => decide(options),
        ("baseline", []) => baseline::regenerate(false).map(|()| SUCCEEDED),
        ("baseline", [flag]) if flag == "--stdout" => baseline::regenerate(true).map(|()| SUCCEEDED),
        ("baseline", [flag]) if flag == "--marker" => {
            println!("{}", baseline::marker()?);
            Ok(SUCCEEDED)
        }
        ("manifest-version", []) => manifest_version(&crate::repository_root().join("Cargo.toml")),
        ("manifest-version", [path]) => manifest_version(Path::new(path)),
        _ => Err(USAGE.into()),
    }
}

/// One text field of the rule's answer; the crate always writes it.
fn answered<'a>(document: &'a Value, key: &str) -> Result<&'a str, String> {
    document[key]
        .as_str()
        .ok_or_else(|| format!("the versioning rule answered no {key}: {document}"))
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
    let document = autoversion::decide(
        &current,
        &surface_file(&published)?,
        &surface_file(&candidate)?,
        breaking,
    )
    .map_err(|refusal| format!("refused ({}): {refusal}", refusal.refusal()))?;
    if as_json {
        let text = serde_json::to_string_pretty(&document).map_err(|error| error.to_string())?;
        println!("{text}");
    } else {
        for key in ["current", "change", "next"] {
            println!("{key}: {}", answered(&document, key)?);
        }
        for key in ["removed", "added"] {
            let names: Vec<&str> = document[key]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(Value::as_str)
                .collect();
            if !names.is_empty() {
                println!("{key}: {}", names.join(", "));
            }
        }
    }
    Ok(SUCCEEDED)
}

/// `[package].version` as Cargo.toml commits it.
fn manifest_version(path: &Path) -> Result<u8, String> {
    let text = fs::read_to_string(path).map_err(|error| format!("{}: {error}", path.display()))?;
    let assignment = Regex::new(r#"^version\s*=\s*"(?P<version>[^"]+)"\s*$"#)
        .map_err(|error| error.to_string())?;
    let mut section = String::new();
    for line in text.lines().map(str::trim) {
        if line.starts_with('[') {
            section = line.to_string();
        } else if section == "[package]" {
            if let Some(captures) = assignment.captures(line) {
                println!("{}", &captures["version"]);
                return Ok(SUCCEEDED);
            }
        }
    }
    Err(format!(
        "{} has no non-empty [package].version",
        path.display()
    ))
}
