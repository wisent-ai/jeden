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

/// Write `value` at `path` inside the object `target`, creating each parent
/// object on the way. A parent that an earlier option already set to a
/// value, or a whole option already given, is refused naming it: replacing
/// it silently dropped what the operator typed first.
fn insert_nested_object(
    target: &mut serde_json::Map<String, Value>,
    path: &[&str],
    value: Value,
) -> Result<(), String> {
    let Some((key, parents)) = path.split_last() else {
        return Err("a browser option names no key".to_string());
    };
    let mut cursor = target;
    for part in parents {
        let next = cursor
            .entry((*part).to_string())
            .or_insert_with(|| json!({}));
        // Read before the mutable borrow below, which lives on as `cursor`.
        if !next.is_object() {
            return Err(format!(
                "browser option {} needs {part} to hold options, but an earlier option set it to {next}",
                path.join(".")
            ));
        }
        let Some(object) = next.as_object_mut() else {
            return Err(format!(
                "browser option {} cannot open {part}",
                path.join(".")
            ));
        };
        cursor = object;
    }
    if let Some(earlier) = cursor.get(*key) {
        return Err(format!(
            "browser option {} is given twice (first as {earlier})",
            path.join(".")
        ));
    }
    cursor.insert((*key).to_string(), value);
    Ok(())
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

/// Which settings an option key writes into, named by the key's first part.
#[derive(Clone, Copy, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
enum Scope {
    Launch,
    Profile,
}

/// Object keys a JavaScript reader of these settings would take for its
/// object machinery rather than an option.
const PROTOTYPE_KEYS: &[&str] = &["__proto__", "constructor", "prototype"];

/// `launch.<key>=value` and `profile.<key>=value`, the forms `/browser`
/// documents; any other key is refused naming those forms.
pub(super) fn parse_browser_options(tokens: &[String]) -> Result<(Value, Value), String> {
    let mut launch = serde_json::Map::new();
    let mut profile = serde_json::Map::new();
    for token in tokens {
        let Some((raw_key, raw_value)) = token.split_once('=') else {
            return Err(format!("Expected key=value option, got \"{token}\"."));
        };
        let key_parts = raw_key
            .split('.')
            .filter(|part| !part.is_empty())
            .collect::<Vec<_>>();
        if key_parts
            .iter()
            .any(|part| PROTOTYPE_KEYS.contains(part) || !valid_option_key_part(part))
        {
            return Err(format!("Invalid browser option key \"{raw_key}\"."));
        }
        let unknown = || {
            format!(
                "Unknown browser option \"{raw_key}\". Use launch.<key>=value or profile.<key>=value."
            )
        };
        let Some((scope, rest)) = key_parts.split_first() else {
            return Err(unknown());
        };
        let scope: Scope =
            crate::vocabulary::named(scope).map_err(|cause| format!("{} ({cause})", unknown()))?;
        let Some(key) = rest.last() else {
            return Err(unknown());
        };
        let target = match scope {
            Scope::Launch => &mut launch,
            Scope::Profile => &mut profile,
        };
        insert_nested_object(target, rest, browser_option_value(key, raw_value))?;
    }
    Ok((Value::Object(launch), Value::Object(profile)))
}
