//! What the schema has to say: the exact objects each envelope kind declares,
//! the fields that must be present, and the shapes the meta, error and replay
//! members take. This is the part that fails when the protocol changes
//! without the contract in protocol/contract.json being told.
//!
//! A contract that does not name what a check compares against is itself an
//! error: read as an empty name, the check passed against nothing.

use super::documents::{object_with_properties, resolved, schema_with_property_const, Document};
use serde_json::{Map, Value};
use std::collections::{BTreeMap, BTreeSet};
use std::num::NonZeroU64;

pub(super) type Located<'a> = (usize, &'a Value);

fn names(value: &Value) -> Vec<String> {
    value
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|item| item.as_str().map(str::to_string))
        .collect()
}

/// The text `key` of the contract part `part`, or an error naming both.
fn named<'a>(part: &'a Value, key: &str, where_: &str, errors: &mut Vec<String>) -> Option<&'a str> {
    let text = part[key].as_str();
    if text.is_none() {
        errors.push(format!("contract: {where_} names no {key}"));
    }
    text
}

/// An object schema with `additionalProperties: false`, whose `required`
/// list is exactly the contract's. When the contract lists `optional` names,
/// the declared properties must be exactly required plus optional; otherwise
/// they must at least include every required name.
fn exact_object(schema: &Value, label: &str, member: &Value, errors: &mut Vec<String>) {
    let Some(object) = schema.as_object() else {
        errors.push(format!("{label}: expected an object schema"));
        return;
    };
    if object.get("type").and_then(Value::as_str) != Some("object") {
        errors.push(format!("{label}: type must be 'object'"));
    }
    if object.get("additionalProperties") != Some(&Value::Bool(false)) {
        errors.push(format!("{label}: additionalProperties must be false"));
    }
    let Some(properties) = object.get("properties").and_then(Value::as_object) else {
        errors.push(format!("{label}: properties must be an object"));
        return;
    };
    let required = names(&member["required"])
        .into_iter()
        .collect::<BTreeSet<_>>();
    let declared = properties.keys().cloned().collect::<BTreeSet<_>>();
    if member["optional"].is_array() {
        let mut expected = required.clone();
        expected.extend(names(&member["optional"]));
        if declared != expected {
            errors.push(format!(
                "{label}: properties are {declared:?}, expected {expected:?}"
            ));
        }
    } else if !required.is_subset(&declared) {
        errors.push(format!(
            "{label}: properties are {declared:?}, which lack {:?}",
            required.difference(&declared).collect::<Vec<_>>()
        ));
    }
    let actual = object.get("required").map(names);
    if actual.map(|list| list.into_iter().collect::<BTreeSet<_>>()) != Some(required.clone()) {
        errors.push(format!(
            "{label}: required is {:?}, expected {required:?}",
            object.get("required")
        ));
    }
}

fn property<'a>(schema: &'a Value, name: &str) -> &'a Value {
    &schema["properties"][name]
}

/// The per-field shapes the contract names for one object schema.
fn field_shapes(
    root: &Value,
    schema: &Value,
    label: &str,
    member: &Value,
    errors: &mut Vec<String>,
) {
    // A non-empty string is the JSON Schema string of minLength 1.
    let non_empty = NonZeroU64::MIN.get();
    for name in names(&member["nonEmpty"]) {
        let field = property(schema, &name);
        if field.is_null() && !names(&member["required"]).contains(&name) {
            continue;
        }
        let field = resolved(root, field);
        if field["type"] != "string" || field["minLength"].as_u64() != Some(non_empty) {
            errors.push(format!(
                "{label}.{name}: must be a string schema with minLength {non_empty}"
            ));
        }
    }
    for name in names(&member["arbitrary"]) {
        if property(schema, &name) != &Value::Object(Map::new()) {
            errors.push(format!(
                "{label}.{name}: must accept arbitrary JSON (expected an empty schema)"
            ));
        }
    }
    for name in names(&member["nonNegativeIntegers"]) {
        let field = resolved(root, property(schema, &name));
        if field["type"] != "integer"
            || !field["minimum"]
                .as_i64()
                .is_some_and(|minimum| !minimum.is_negative())
        {
            errors.push(format!(
                "{label}.{name}: must be an integer with a minimum that is not negative"
            ));
        }
    }
    for name in names(&member["booleans"]) {
        if resolved(root, property(schema, &name))["type"] != "boolean" {
            errors.push(format!("{label}.{name}: must be boolean"));
        }
    }
}

/// A member object such as the request meta, reached through the envelope
/// property the contract names; a contract member that names no property is
/// an error, not a member silently left unchecked.
fn member_object(
    documents: &[Document],
    envelope: Located<'_>,
    member: &Value,
    label: &str,
    errors: &mut Vec<String>,
) -> Option<Value> {
    let (index, envelope_schema) = envelope;
    let root = &documents[index].value;
    let name = named(member, "property", label, errors)?;
    let schema = resolved(root, property(envelope_schema, name));
    exact_object(schema, label, member, errors);
    field_shapes(root, schema, label, member, errors);
    Some(schema.clone())
}

pub(super) fn schema_contract<'a>(
    documents: &'a [Document],
    contract: &Value,
    errors: &mut Vec<String>,
) -> BTreeMap<String, Located<'a>> {
    let mut schemas = BTreeMap::new();
    let protocol = named(contract, "protocol", "the contract", errors);
    let envelopes = match contract["envelopes"].as_object() {
        Some(envelopes) => envelopes.clone(),
        None => {
            errors.push("contract: envelopes is not an object of envelope kinds".into());
            Map::new()
        }
    };
    for (kind, member) in &envelopes {
        let Some(found) = schema_with_property_const(documents, "type", kind) else {
            errors.push(format!(
                "schema: no '{kind}' envelope with properties.type.const == '{kind}'"
            ));
            continue;
        };
        let (index, envelope_schema) = found;
        let label = format!("schema {kind} envelope");
        exact_object(envelope_schema, &label, member, errors);
        field_shapes(
            &documents[index].value,
            envelope_schema,
            &format!("schema {kind}"),
            member,
            errors,
        );
        schemas.insert(kind.clone(), found);
    }
    if let Some(&request) = schemas.get("request") {
        let meta_contract = &contract["requestMeta"];
        if let Some(meta) = member_object(
            documents,
            request,
            meta_contract,
            "schema request.meta",
            errors,
        ) {
            let constant = named(meta_contract, "protocolConstant", "requestMeta", errors);
            if let (Some(constant), Some(protocol)) = (constant, protocol) {
                if meta["properties"][constant]["const"].as_str() != Some(protocol) {
                    errors.push(format!(
                        "schema request.meta.{constant}: const must be {protocol:?}"
                    ));
                }
            }
        }
    }
    if let Some(&error) = schemas.get("error") {
        member_object(
            documents,
            error,
            &contract["errorPayload"],
            "schema error.error",
            errors,
        );
    }
    let replay = &contract["replay"];
    let Some(method) = named(replay, "method", "replay", errors) else {
        return schemas;
    };
    if schema_with_property_const(documents, "method", method).is_none() {
        errors.push(format!(
            "schema: no request specialization with method const '{method}'"
        ));
    }
    let required = names(&replay["required"]);
    let optional = names(&replay["optional"]);
    let wanted = required.iter().chain(optional.iter()).collect::<Vec<_>>();
    match object_with_properties(documents, &wanted) {
        None => errors.push(format!("schema: no {method} params object with {wanted:?}")),
        Some((index, params)) => {
            exact_object(params, &format!("schema {method} params"), replay, errors);
            field_shapes(
                &documents[index].value,
                params,
                "schema replay",
                replay,
                errors,
            );
        }
    }
    schemas
}
