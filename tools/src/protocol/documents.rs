//! Reading the schema documents: loading them, walking them, and finding the
//! one schema that describes a given kind of envelope.

use serde_json::Value;
use std::fs;
use std::path::{Path, PathBuf};

pub(super) struct Document {
    pub(super) path: PathBuf,
    pub(super) value: Value,
}

fn json_files(directory: &Path, found: &mut Vec<PathBuf>) -> Result<(), String> {
    let entries =
        fs::read_dir(directory).map_err(|error| format!("{}: {error}", directory.display()))?;
    for entry in entries {
        let path = entry.map_err(|error| error.to_string())?.path();
        if path.is_dir() {
            json_files(&path, found)?;
        } else if path
            .extension()
            .is_some_and(|extension| extension == "json")
        {
            found.push(path);
        }
    }
    Ok(())
}

pub(super) fn load(directory: &Path, errors: &mut Vec<String>) -> Vec<Document> {
    if !directory.is_dir() {
        errors.push(format!(
            "schema directory is missing: {}",
            directory.display()
        ));
        return Vec::new();
    }
    let mut paths = Vec::new();
    if let Err(error) = json_files(directory, &mut paths) {
        errors.push(format!("cannot list {}: {error}", directory.display()));
    }
    paths.sort();
    let mut documents = Vec::new();
    for path in paths {
        let parsed = fs::read(&path)
            .map_err(|error| error.to_string())
            .and_then(|bytes| serde_json::from_slice(&bytes).map_err(|error| error.to_string()));
        match parsed {
            Ok(value) => documents.push(Document { path, value }),
            Err(error) => errors.push(format!("cannot read JSON {}: {error}", path.display())),
        }
    }
    documents
}

/// Every node of a JSON value, the value itself first.
pub(super) fn walk(value: &Value) -> Vec<&Value> {
    let mut nodes = vec![value];
    let mut index = 0;
    while index < nodes.len() {
        let node = nodes[index];
        match node {
            Value::Object(map) => nodes.extend(map.values()),
            Value::Array(items) => nodes.extend(items.iter()),
            _ => {}
        }
        index += 1;
    }
    nodes
}

fn property_count(node: &Value) -> usize {
    node["properties"]
        .as_object()
        .map_or(0, |properties| properties.len())
}

/// The richest object schema whose `properties.<name>.const` equals
/// `constant`, with the index of the document that owns it.
pub(super) fn schema_with_property_const<'a>(
    documents: &'a [Document],
    name: &str,
    constant: &str,
) -> Option<(usize, &'a Value)> {
    let mut best: Option<(usize, &Value)> = None;
    for (index, document) in documents.iter().enumerate() {
        for node in walk(&document.value) {
            let matches = node["properties"].is_object()
                && node["properties"][name]["const"].as_str() == Some(constant);
            if matches
                && best.is_none_or(|(_, current)| property_count(node) > property_count(current))
            {
                best = Some((index, node));
            }
        }
    }
    best
}

/// The first object schema whose property names are exactly `names`.
pub(super) fn object_with_properties<'a>(
    documents: &'a [Document],
    names: &[&String],
) -> Option<(usize, &'a Value)> {
    let mut wanted = names.iter().map(|name| name.as_str()).collect::<Vec<_>>();
    wanted.sort_unstable();
    wanted.dedup();
    documents.iter().enumerate().find_map(|(index, document)| {
        walk(&document.value).into_iter().find_map(|node| {
            let properties = node["properties"].as_object()?;
            let mut present = properties.keys().map(String::as_str).collect::<Vec<_>>();
            present.sort_unstable();
            (node["type"] == "object" && present == wanted).then_some((index, node))
        })
    })
}

/// Follow `$ref` chains inside the owning document until a schema that is not
/// a reference, or a reference that cannot be followed.
pub(super) fn resolved<'a>(root: &'a Value, schema: &'a Value) -> &'a Value {
    let mut current = schema;
    let mut seen = Vec::new();
    while let Some(reference) = current["$ref"].as_str() {
        if seen.contains(&reference) {
            return current;
        }
        seen.push(reference);
        match pointer(root, reference) {
            Ok(target) => current = target,
            Err(_) => return current,
        }
    }
    current
}

/// A local JSON pointer reference such as `#/$defs/request`.
pub(super) fn pointer<'a>(document: &'a Value, fragment: &str) -> Result<&'a Value, String> {
    let Some(pointer) = fragment.strip_prefix('#') else {
        return Err(format!("unsupported non-local reference {fragment:?}"));
    };
    if pointer.is_empty() {
        return Ok(document);
    }
    if !pointer.starts_with('/') {
        return Err(format!("unsupported JSON pointer {fragment:?}"));
    }
    document
        .pointer(pointer)
        .ok_or_else(|| format!("unresolved JSON pointer {fragment:?}"))
}
