//! Connecting to every configured server at once, and collecting what they
//! offer.
//!
//! Split out of `mcp/mod.rs`, which had grown past the module line cap.

use super::connection::{listed, ServerConnection};
use super::servers::{configured_server, configured_servers};
use super::{managers, session_key, with_connection};
use crate::mcp::connection::ConnectionState;
use serde_json::{json, Value};
use std::collections::BTreeSet;
use std::path::Path;
use std::thread;

/// One handshake per server: its name, the connection, and how the handshake
/// went.
type Handshakes = Vec<(String, ServerConnection, Result<(), String>)>;

/// Every pending server's handshake, each on its own thread: a handshake waits
/// on a child process, so one server's slow start never delays another's.
fn connect_parallel(
    pending: Vec<(String, ServerConnection)>,
    cwd: &Path,
    force: bool,
) -> Result<Handshakes, String> {
    thread::scope(|scope| {
        let handles = pending
            .into_iter()
            .map(|(name, mut connection)| {
                let server = name.clone();
                let handle = scope.spawn(move || {
                    let result = connection.connect(cwd, force);
                    (name, connection, result)
                });
                (server, handle)
            })
            .collect::<Vec<_>>();
        handles
            .into_iter()
            .map(|(server, handle)| {
                handle.join().map_err(|panic| {
                    crate::panicked::refusal(
                        &format!("the startup worker of MCP server {server}"),
                        &*panic,
                    )
                })
            })
            .collect()
    })
}

pub fn live_tools(cwd: &Path) -> Result<Vec<(String, Value)>, String> {
    let key = session_key(cwd);
    let pending = {
        let mut managers = managers()?;
        let manager = managers.entry(key.clone()).or_default();
        manager.sync_config(cwd)?;
        for connection in manager.servers.values_mut() {
            let notifications = match connection.client.as_mut() {
                Some(client) => {
                    if client.is_alive() {
                        client.poll_notifications()
                    } else {
                        continue;
                    }
                }
                None => continue,
            };
            match notifications {
                Ok(notifications) => {
                    if let Err(error) = connection.process_notifications(cwd, notifications) {
                        connection.record_failure(error);
                    }
                }
                Err(error) => connection.record_failure(error),
            }
        }
        let names = manager
            .servers
            .iter_mut()
            .filter_map(|(name, connection)| {
                let alive = match connection.client.as_mut() {
                    Some(client) => client.is_alive(),
                    None => false,
                };
                (!alive).then(|| name.clone())
            })
            .collect::<Vec<_>>();
        names
            .into_iter()
            .filter_map(|name| {
                manager
                    .servers
                    .remove(&name)
                    .map(|connection| (name, connection))
            })
            .collect::<Vec<_>>()
    };
    let results = connect_parallel(pending, cwd, false)?;
    let mut startup_errors = Vec::new();
    let mut managers = managers()?;
    let manager = managers.entry(key).or_default();
    for (name, connection, result) in results {
        if let Err(error) = result {
            startup_errors.push(format!("{name}: {error}"));
        }
        manager.servers.insert(name, connection);
    }
    let tools = manager
        .servers
        .iter()
        .filter(|(_, connection)| matches!(connection.state, ConnectionState::Ready))
        .flat_map(|(server, connection)| {
            connection
                .tools
                .get("tools")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
                .cloned()
                .map(move |tool| (server.clone(), tool))
        })
        .collect::<Vec<_>>();
    if tools.is_empty() && !startup_errors.is_empty() {
        return Err(format!(
            "no MCP server reached ready state: {}",
            startup_errors.join("; ")
        ));
    }
    Ok(tools)
}

pub fn refresh_all(cwd: &Path) -> Result<Value, String> {
    let configs = configured_servers(cwd)?;
    let pending = configs
        .into_iter()
        .map(|(name, config)| (name, ServerConnection::new(config)))
        .collect();
    let results = connect_parallel(pending, cwd, true)?;
    let key = session_key(cwd);
    let mut managers = managers()?;
    let manager = managers.entry(key).or_default();
    let mut status = serde_json::Map::new();
    let configured_names = results
        .iter()
        .map(|(name, _, _)| name.clone())
        .collect::<BTreeSet<_>>();
    manager
        .servers
        .retain(|name, _| configured_names.contains(name));
    for (name, connection, result) in results {
        status.insert(name.clone(), match result { Ok(()) => json!({"state": "ready", "tools": listed(&connection.tools, "tools")}), Err(error) => json!({"state": connection.state.as_str(), "error": error}) });
        manager.servers.insert(name, connection);
    }
    crate::capability::invalidate();
    Ok(Value::Object(status))
}

pub fn reconnect(cwd: &Path, server_name: &str) -> Result<Value, String> {
    let config = configured_server(cwd, server_name)?;
    with_connection(cwd, server_name, |connection| {
        connection.disconnect();
        connection.config = config;
        connection.failures.clear();
        connection.connect(cwd, true)?;
        crate::capability::invalidate();
        Ok(
            json!({"server": server_name, "state": connection.state.as_str(), "tools": listed(&connection.tools, "tools")}),
        )
    })
}
