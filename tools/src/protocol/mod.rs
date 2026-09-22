//! The jeden.session.v1 compatibility gate: the canonical schema held to the
//! contract in protocol/contract.json, the golden envelopes validated against
//! the schema, and each SDK checked for the protocol constant and the field
//! spellings the wire uses.

mod checks;
mod documents;
mod golden;

use crate::release::digest_file;
use documents::Document;
use regex::Regex;
use serde_json::{json, Map, Value};
use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

const CONTRACT_FILE: &str = "protocol/contract.json";
const USAGE: &str = "usage: jeden-tools protocol-check [--root ROOT] [--print-manifest]";

pub(crate) fn run(arguments: &[String]) -> Result<u8, String> {
    let mut root = crate::repository_root();
    let mut print_manifest = false;
    let mut remaining = arguments.iter();
    while let Some(option) = remaining.next() {
        match option.as_str() {
            "--root" => root = remaining.next().ok_or(USAGE)?.into(),
            "--print-manifest" => print_manifest = true,
            _ => return Err(USAGE.into()),
        }
    }
    let root = fs::canonicalize(&root).map_err(|error| format!("{}: {error}", root.display()))?;
    let (errors, manifest) = check(&root)?;
    if !errors.is_empty() {
        eprintln!(
            "jeden.session.v1 compatibility check failed ({} issue(s)):",
            errors.len()
        );
        for error in &errors {
            eprintln!("  - {error}");
        }
        eprintln!(
            "Invocation: jeden-tools protocol-check --root {}",
            root.display()
        );
        return Ok(1);
    }
    if print_manifest {
        println!(
            "{}",
            serde_json::to_string_pretty(&manifest).map_err(|error| error.to_string())?
        );
    } else {
        let sources = manifest["sdkSources"].as_object().map_or(0, |groups| {
            groups
                .values()
                .filter_map(Value::as_array)
                .map(Vec::len)
                .sum()
        });
        println!(
            "jeden.session.v1 compatibility check passed: {} JSON document(s), {sources} SDK source file(s)",
            manifest["json"].as_array().map_or(0, Vec::len)
        );
    }
    Ok(0)
}

/// A section the contract must carry, refused by name when it is absent.
fn section<'a>(contract: &'a Value, key: &str) -> Result<&'a Map<String, Value>, String> {
    contract[key]
        .as_object()
        .ok_or_else(|| format!("{CONTRACT_FILE}: `{key}` must be an object"))
}

fn text<'a>(value: &'a Value, what: &str) -> Result<&'a str, String> {
    value
        .as_str()
        .ok_or_else(|| format!("{CONTRACT_FILE}: {what} must be a string"))
}

fn check(root: &Path) -> Result<(Vec<String>, Value), String> {
    let path = root.join(CONTRACT_FILE);
    let body = fs::read_to_string(&path).map_err(|error| format!("{CONTRACT_FILE}: {error}"))?;
    let contract: Value =
        serde_json::from_str(&body).map_err(|error| format!("{CONTRACT_FILE}: {error}"))?;
    let protocol = text(&contract["protocol"], "`protocol`")?;
    section(&contract, "envelopes")?;
    section(&contract, "requestMeta")?;
    section(&contract, "errorPayload")?;
    section(&contract, "replay")?;
    let sdk_contracts = section(&contract, "sdks")?;
    let mut errors = Vec::new();
    let directory = root.join(text(&contract["schemaDirectory"], "`schemaDirectory`")?);
    let documents = documents::load(&directory, &mut errors);
    if !documents.is_empty() {
        let schemas = checks::schema_contract(&documents, &contract, &mut errors);
        golden::check(&documents, &schemas, &mut errors);
    }
    let mut groups = BTreeMap::new();
    for (language, sdk) in sdk_contracts {
        groups.insert(
            language.clone(),
            check_sdk(root, protocol, language, sdk, &mut errors)?,
        );
    }
    let manifest = manifest(root, &contract, &documents, &groups)?;
    errors.sort();
    errors.dedup();
    Ok((errors, manifest))
}

fn source_files(
    directory: &Path,
    suffixes: &[String],
    found: &mut Vec<PathBuf>,
) -> Result<(), String> {
    if !directory.is_dir() {
        return Ok(());
    }
    let entries =
        fs::read_dir(directory).map_err(|error| format!("{}: {error}", directory.display()))?;
    for entry in entries {
        let path = entry.map_err(|error| error.to_string())?.path();
        let name = path
            .file_name()
            .map(|name| name.to_string_lossy().into_owned());
        if path.is_dir() {
            source_files(&path, suffixes, found)?;
        } else if name.is_some_and(|name| {
            suffixes
                .iter()
                .any(|suffix| name.ends_with(suffix.as_str()))
        }) {
            found.push(path);
        }
    }
    Ok(())
}

fn strings(value: &Value) -> Vec<String> {
    value
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|item| item.as_str().map(str::to_string))
        .collect()
}

fn check_sdk(
    root: &Path,
    protocol: &str,
    language: &str,
    sdk: &Value,
    errors: &mut Vec<String>,
) -> Result<Vec<PathBuf>, String> {
    let mut paths = Vec::new();
    let directory = root.join(text(&sdk["path"], &format!("{language} path"))?);
    source_files(&directory, &strings(&sdk["suffixes"]), &mut paths)?;
    paths.sort();
    if paths.is_empty() {
        errors.push(format!(
            "{language} SDK: no source files found at its canonical SDK path"
        ));
        return Ok(paths);
    }
    let mut source = String::new();
    for path in &paths {
        match fs::read_to_string(path) {
            Ok(content) => {
                source.push_str(&content);
                source.push('\n');
            }
            Err(error) => errors.push(format!(
                "{language} SDK: cannot read {}: {error}",
                path.display()
            )),
        }
    }
    if !source.contains(protocol) {
        errors.push(format!(
            "{language} SDK: protocol constant value {protocol:?} not found"
        ));
        return Ok(paths);
    }
    let constant = Regex::new(text(
        &sdk["protocolConstant"],
        &format!("{language} protocolConstant"),
    )?)
    .map_err(|error| format!("{CONTRACT_FILE}: {language} protocolConstant: {error}"))?;
    if !constant.is_match(&source) {
        errors.push(format!(
            "{language} SDK: expected a protocol const equal to {protocol:?}"
        ));
    }
    let camel_case = match sdk["camelCasePolicy"].as_str() {
        Some(policy) => Regex::new(policy)
            .map_err(|error| error.to_string())?
            .is_match(&source),
        None => false,
    };
    for (camel, snake) in sdk["fields"].as_object().into_iter().flatten() {
        let snake = text(snake, &format!("{language} field {camel}"))?;
        if !source.contains(snake) {
            errors.push(format!(
                "{language} SDK: field {snake:?} (JSON {camel:?}) not found"
            ));
        }
        if !camel_case && !source.contains(&format!("\"{camel}\"")) {
            errors.push(format!(
                "{language} SDK: no serde camelCase policy or explicit spelling {camel:?}"
            ));
        }
    }
    for field in strings(&sdk["wordFields"]) {
        let word = Regex::new(&format!(r"\b{}\b", regex::escape(&field)))
            .map_err(|error| error.to_string())?;
        if !word.is_match(&source) {
            errors.push(format!(
                "{language} SDK: JSON field spelling {field:?} not found"
            ));
        }
    }
    Ok(paths)
}

fn relative(root: &Path, path: &Path) -> String {
    path.strip_prefix(root)
        .unwrap_or(path)
        .display()
        .to_string()
}

fn manifest(
    root: &Path,
    contract: &Value,
    documents: &[Document],
    groups: &BTreeMap<String, Vec<PathBuf>>,
) -> Result<Value, String> {
    let mut json_documents = Vec::new();
    for document in documents {
        json_documents.push(json!({
            "path": relative(root, &document.path),
            "sha256": digest_file(&document.path)?,
        }));
    }
    let envelopes = section(contract, "envelopes")?
        .iter()
        .map(|(kind, member)| (kind.clone(), member["required"].clone()))
        .collect::<Map<_, _>>();
    let sources = groups
        .iter()
        .map(|(language, paths)| {
            (
                language.clone(),
                json!(paths
                    .iter()
                    .map(|path| relative(root, path))
                    .collect::<Vec<_>>()),
            )
        })
        .collect::<Map<_, _>>();
    Ok(json!({
        "protocol": contract["protocol"],
        "envelopes": envelopes,
        "json": json_documents,
        "sdkSources": sources,
    }))
}
