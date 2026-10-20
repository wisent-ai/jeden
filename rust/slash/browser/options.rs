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
        insert_nested_object(target, rest, browser_option_value(key, raw_value));
    }
    Ok((launch, profile))
}
