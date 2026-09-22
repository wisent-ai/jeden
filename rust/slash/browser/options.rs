//! Reading the launch and profile options an operator typed on one line, and
//! turning them into the nested settings the browser runtime expects.
//!
//! Split out of `slash/browser.rs`, which had grown past the module line cap.

use serde_json::{json, Value};

fn browser_option_value(key: &str, value: &str) -> Value {
    if value == "true" {
        return json!(true);
    }
    if value == "false" {
        return json!(false);
    }
    if key == "slowMo" {
        if let Ok(number) = value.parse::<f64>() {
            return json!(number);
        }
    }
    if key == "args" {
        return json!(value
            .split(',')
            .map(str::trim)
            .filter(|part| !part.is_empty())
            .collect::<Vec<_>>());
    }
    json!(value)
}

fn insert_nested_object(target: &mut Value, path: &[&str], value: Value) {
    if !target.is_object() {
        *target = json!({});
    }
    let Some((key, parents)) = path.split_last() else {
        return;
    };
    let mut cursor = target.as_object_mut().expect("object");
    for part in parents {
        let next = cursor
            .entry((*part).to_string())
            .or_insert_with(|| json!({}));
        if !next.is_object() {
            *next = json!({});
        }
        cursor = next.as_object_mut().expect("nested object");
    }
    cursor.insert((*key).to_string(), value);
}

fn valid_option_key_part(part: &str) -> bool {
    let mut chars = part.chars();
    match chars.next() {
        Some(first) => {
            first.is_ascii_alphabetic()
                && chars.all(|ch| ch.is_ascii_alphanumeric() || ch == '_' || ch == '-')
        }
        None => false,
    }
}

pub(super) fn parse_browser_options(tokens: &[String]) -> Result<(Value, Value), String> {
    let mut launch = json!({});
    let mut profile = json!({});
    for token in tokens {
        let Some((raw_key, raw_value)) = token.split_once('=') else {
            return Err(format!("Expected key=value option, got \"{token}\"."));
        };
        let key_parts = raw_key
            .split('.')
            .filter(|part| !part.is_empty())
            .collect::<Vec<_>>();
        let Some((scope, rest)) = key_parts.split_first() else {
            return Err(format!("Invalid browser option key \"{raw_key}\"."));
        };
        let scope = *scope;
        if key_parts.iter().any(|part| {
            matches!(*part, "__proto__" | "constructor" | "prototype")
                || !valid_option_key_part(part)
        }) {
            return Err(format!("Invalid browser option key \"{raw_key}\"."));
        }
        if scope == "launch" && !rest.is_empty() {
            insert_nested_object(
                &mut launch,
                rest,
                browser_option_value(rest.last().copied().unwrap_or(""), raw_value),
            );
        } else if scope == "profile" && !rest.is_empty() {
            insert_nested_object(
                &mut profile,
                rest,
                browser_option_value(rest.last().copied().unwrap_or(""), raw_value),
            );
        } else if raw_key == "launch" {
            insert_nested_object(
                &mut launch,
                &["executablePath"],
                browser_option_value("executablePath", raw_value),
            );
        } else if raw_key == "profile" {
            insert_nested_object(
                &mut profile,
                &["name"],
                browser_option_value("name", raw_value),
            );
        } else if matches!(
            raw_key,
            "args" | "channel" | "devtools" | "executablePath" | "slowMo"
        ) {
            insert_nested_object(
                &mut launch,
                &[raw_key],
                browser_option_value(raw_key, raw_value),
            );
        } else if matches!(raw_key, "name" | "profileDir" | "profile" | "userDataDir") {
            let normalized = if raw_key == "profileDir" {
                "userDataDir"
            } else {
                raw_key
            };
            insert_nested_object(
                &mut profile,
                &[normalized],
                browser_option_value(normalized, raw_value),
            );
        } else {
            return Err(format!("Unknown browser option \"{raw_key}\". Use launch.<key>=value or profile.<key>=value."));
        }
    }
    Ok((launch, profile))
}
