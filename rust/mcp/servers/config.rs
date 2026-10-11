//! Which external tool servers this workspace has, merged from the project
//! and the account and honouring what is turned off.
//!
//! Split out of `mcp/mod.rs`, which had grown past the module line cap.

use serde_json::{json, Value};
use std::collections::BTreeMap;
use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

pub(super) use crate::slash::common::dirs_home;
use crate::slash::common::read_json_value;

/// The user's and the project's `mcp.json` merged. One that cannot be read,
/// does not parse, or declares something of the wrong shape is refused naming
/// the file: read as nothing, every server it declares and every server it
/// disables vanished without a word, and an `enabled` that was not a boolean
/// left a server its file meant to turn off running.
pub fn load_config(cwd: &Path) -> Result<Value, String> {
    let mut servers = serde_json::Map::new();
    let mut disabled = Vec::new();
    let mut lists = Vec::new();
    for path in [
        dirs_home().join(".jeden/mcp.json"),
        cwd.join(".jeden/mcp.json"),
    ] {
        let (declared, listed) = declarations(&path)?;
        servers.extend(declared);
        lists.push(listed);
    }
    for (name, server) in &servers {
        if server.get("enabled").and_then(Value::as_bool) == Some(false) {
            disabled.push(name.clone());
        }
    }
    disabled.extend(lists.into_iter().flatten());
    Ok(json!({"mcpServers": servers, "disabledServers": disabled}))
}

/// One `mcp.json`: its servers and the names it disables. A missing file
/// declares nothing; a root that is not an object, `mcpServers` that is not
/// an object of objects, an `enabled` that is not a boolean, or a
/// `disabledServers` that is not a list of names is refused with the file,
/// the field and what it held.
fn declarations(path: &Path) -> Result<(serde_json::Map<String, Value>, Vec<String>), String> {
    let source = read_json_value(path)?;
    let file = path.display();
    let root = source
        .as_object()
        .ok_or_else(|| format!("{file} does not hold a JSON object"))?;
    let servers = match root.get("mcpServers") {
        None => serde_json::Map::new(),
        Some(Value::Object(servers)) => servers.clone(),
        Some(other) => {
            return Err(format!(
                "{file}: mcpServers must be an object of servers, not {other}"
            ))
        }
    };
    for (name, server) in &servers {
        if !server.is_object() {
            return Err(format!(
                "{file}: mcpServers.{name} must be an object, not {server}"
            ));
        }
        match server.get("enabled") {
            None | Some(Value::Bool(_)) => {}
            Some(other) => {
                return Err(format!(
                    "{file}: mcpServers.{name}.enabled must be true or false, not {other}"
                ))
            }
        }
    }
    let disabled = match root.get("disabledServers") {
        None => Vec::new(),
        Some(Value::Array(values)) => values
            .iter()
            .map(|value| {
                value.as_str().map(ToOwned::to_owned).ok_or_else(|| {
                    format!("{file}: disabledServers holds {value}, which is not a server name")
                })
            })
            .collect::<Result<Vec<_>, _>>()?,
        Some(other) => {
            return Err(format!(
                "{file}: disabledServers must be a list of server names, not {other}"
            ))
        }
    };
    Ok((servers, disabled))
}

pub(crate) fn configured_servers(cwd: &Path) -> Result<BTreeMap<String, Value>, String> {
    let config = load_config(cwd)?;
    let disabled = config
        .get("disabledServers")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .collect::<BTreeSet<_>>();
    let servers = config
        .get("mcpServers")
        .and_then(Value::as_object)
        .ok_or("mcpServers must be an object")?;
    Ok(servers
        .iter()
        .filter(|(name, _)| !disabled.contains(name.as_str()))
        .map(|(name, server)| (name.clone(), server.clone()))
        .collect())
}

pub(crate) fn configured_server(cwd: &Path, server_name: &str) -> Result<Value, String> {
    let config = load_config(cwd)?;
    let disabled = config
        .get("disabledServers")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .collect::<BTreeSet<_>>();
    if disabled.contains(server_name) {
        return Err(format!("disabled MCP server: {server_name}"));
    }
    config
        .get("mcpServers")
        .and_then(Value::as_object)
        .and_then(|servers| servers.get(server_name))
        .cloned()
        .ok_or_else(|| format!("unknown MCP server: {server_name}"))
}

pub(crate) fn string_field<'a>(value: &'a Value, key: &str) -> Option<&'a str> {
    value.get(key).and_then(Value::as_str)
}

pub(crate) fn resolve_server_cwd(cwd: &Path, server: &Value) -> PathBuf {
    match string_field(server, "cwd") {
        Some(raw) if Path::new(raw).is_absolute() => PathBuf::from(raw),
        Some(raw) => cwd.join(raw),
        None => cwd.to_path_buf(),
    }
}
