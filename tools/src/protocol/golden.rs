//! Validating a document against the subset of JSON Schema (Draft 2020-12)
//! the protocol uses, and holding the golden envelopes to it.

use super::checks::Located;
use super::documents::{pointer, walk, Document};
use regex::Regex;
use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet};

fn type_name(value: &Value) -> &'static str {
    match value {
        Value::Null => "null",
        Value::Bool(_) => "boolean",
        Value::Number(number) if number.is_i64() || number.is_u64() => "integer",
        Value::Number(_) => "number",
        Value::String(_) => "string",
        Value::Array(_) => "array",
        Value::Object(_) => "object",
    }
}

fn type_matches(expected: &str, value: &Value) -> bool {
    let actual = type_name(value);
    actual == expected || (expected == "number" && actual == "integer")
}

pub(super) fn validate(value: &Value, schema: &Value, root: &Value, location: &str) -> Vec<String> {
    let Some(object) = schema.as_object() else {
        return match schema {
            Value::Bool(true) => Vec::new(),
            Value::Bool(false) => vec![format!("{location}: schema is false")],
            other => vec![format!("{location}: invalid schema node {other}")],
        };
    };
    if let Some(reference) = object.get("$ref").and_then(Value::as_str) {
        return match pointer(root, reference) {
            Ok(target) => validate(value, target, root, location),
            Err(error) => vec![format!("{location}: {error}")],
        };
    }
    let mut failures = Vec::new();
    for child in object
        .get("allOf")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
    {
        failures.extend(validate(value, child, root, location));
    }
    if let Some(alternatives) = object.get("oneOf").and_then(Value::as_array) {
        let matched = alternatives
            .iter()
            .filter(|child| validate(value, child, root, location).is_empty())
            .count();
        if matched != 1 {
            failures.push(format!(
                "{location}: expected exactly one oneOf alternative to match"
            ));
        }
    }
    if let Some(alternatives) = object.get("anyOf").and_then(Value::as_array) {
        if !alternatives
            .iter()
            .any(|child| validate(value, child, root, location).is_empty())
        {
            failures.push(format!("{location}: no anyOf alternative matched"));
        }
    }
    if let Some(condition) = object.get("if") {
        let branch = if validate(value, condition, root, location).is_empty() {
            object.get("then")
        } else {
            object.get("else")
        };
        if let Some(branch) = branch {
            failures.extend(validate(value, branch, root, location));
        }
    }
    if let Some(constant) = object.get("const") {
        if value != constant {
            failures.push(format!(
                "{location}: expected constant {constant}, got {value}"
            ));
        }
    }
    if let Some(allowed) = object.get("enum").and_then(Value::as_array) {
        if !allowed.contains(value) {
            failures.push(format!(
                "{location}: {value} is not in enum {}",
                Value::Array(allowed.clone())
            ));
        }
    }
    if let Some(expected) = object.get("type").and_then(Value::as_str) {
        if !type_matches(expected, value) {
            failures.push(format!(
                "{location}: expected type {expected}, got {}",
                type_name(value)
            ));
            return failures;
        }
    }
    match value {
        Value::Object(members) => {
            for name in object
                .get("required")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
            {
                if name
                    .as_str()
                    .is_some_and(|name| !members.contains_key(name))
                {
                    failures.push(format!("{location}: missing required property {name}"));
                }
            }
            let properties = object.get("properties").and_then(Value::as_object);
            let additional = object.get("additionalProperties");
            for (name, child) in members {
                let child_location = format!("{location}.{name}");
                match (
                    properties.and_then(|properties| properties.get(name)),
                    additional,
                ) {
                    (Some(declared), _) => {
                        failures.extend(validate(child, declared, root, &child_location))
                    }
                    (None, Some(Value::Bool(false))) => {
                        failures.push(format!("{location}: unknown property {name:?}"))
                    }
                    (None, Some(extra @ Value::Object(_))) => {
                        failures.extend(validate(child, extra, root, &child_location))
                    }
                    _ => {}
                }
            }
        }
        Value::Array(items) => {
            if let Some(item_schema @ Value::Object(_)) = object.get("items") {
                for (index, child) in items.iter().enumerate() {
                    failures.extend(validate(
                        child,
                        item_schema,
                        root,
                        &format!("{location}[{index}]"),
                    ));
                }
            }
        }
        Value::String(text) => {
            if let Some(minimum) = object.get("minLength").and_then(Value::as_u64) {
                if (text.chars().count() as u64) < minimum {
                    failures.push(format!(
                        "{location}: string is shorter than minLength {minimum}"
                    ));
                }
            }
            if let Some(pattern) = object.get("pattern").and_then(Value::as_str) {
                match Regex::new(pattern) {
                    Ok(expression) if expression.is_match(text) => {}
                    Ok(_) => failures.push(format!(
                        "{location}: string does not match pattern {pattern:?}"
                    )),
                    Err(error) => {
                        failures.push(format!("{location}: invalid pattern {pattern:?}: {error}"))
                    }
                }
            }
        }
        Value::Number(number) => {
            let actual = number.as_f64();
            if let (Some(actual), Some(minimum)) =
                (actual, object.get("minimum").and_then(Value::as_f64))
            {
                if actual < minimum {
                    failures.push(format!("{location}: value is less than minimum {minimum}"));
                }
            }
            if let (Some(actual), Some(maximum)) =
                (actual, object.get("maximum").and_then(Value::as_f64))
            {
                if actual > maximum {
                    failures.push(format!(
                        "{location}: value is greater than maximum {maximum}"
                    ));
                }
            }
        }
        _ => {}
    }
    failures
}

/// Every envelope in the golden documents (those that are not schemas) is
/// valid against the schema of its kind, and every kind has one.
pub(super) fn check(
    documents: &[Document],
    schemas: &BTreeMap<String, Located<'_>>,
    errors: &mut Vec<String>,
) {
    let mut seen = BTreeSet::new();
    for document in documents {
        if document.value.get("$schema").is_some() {
            continue;
        }
        let file = document
            .path
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_else(|| document.path.display().to_string());
        for node in walk(&document.value) {
            let Some(kind) = node.get("type").and_then(Value::as_str) else {
                continue;
            };
            let Some(&(owner, schema)) = schemas.get(kind) else {
                continue;
            };
            seen.insert(kind.to_string());
            let label = format!("{file}:{kind}");
            for failure in validate(node, schema, &documents[owner].value, &label) {
                errors.push(format!("golden validation: {failure}"));
            }
        }
    }
    for kind in schemas.keys() {
        if !seen.contains(kind) {
            errors.push(format!("golden fixtures: missing '{kind}' envelope"));
        }
    }
}
