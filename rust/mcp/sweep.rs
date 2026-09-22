//! Connecting to every configured server at once, and collecting what they
//! offer.
//!
//! Split out of `mcp/mod.rs`, which had grown past the module line cap.

use super::servers::{configured_server, configured_servers};
use super::connection::ServerConnection;
use super::{managers, session_key, with_connection};
use serde_json::{json, Value};
use std::collections::BTreeSet;
use std::path::Path;
use std::sync::{Arc, Mutex};
use std::thread;
use crate::mcp::client::McpClient;
use crate::mcp::connection::ConnectionState;
use std::collections::VecDeque;

/// One handshake per server: its name, the connection, and how the handshake
/// went.
type Handshakes = Vec<(String, ServerConnection, Result<(), String>)>;

fn connect_parallel(
    pending: Vec<(String, ServerConnection)>,
    cwd: &Path,
    force: bool,
) -> Result<Handshakes, String> {
    if pending.is_empty() {
        return Ok(Vec::new());
    }
    let worker_count = pending
        .len()
        .min(thread::available_parallelism().map_or(1, usize::from))
        .min(8);
    let queue = Arc::new(Mutex::new(VecDeque::from(pending)));
    thread::scope(|scope| {
        let handles = (0..worker_count)
            .map(|_| {
                let queue = Arc::clone(&queue);
                scope.spawn(move || {
                    let mut completed = Vec::new();
                    loop {
                        let item = queue
                            .lock()
                            .map_err(|_| "MCP startup queue lock is poisoned".to_string())?
                            .pop_front();
                        let Some((name, mut connection)) = item else {
                            break;
                        };
                        let result = connection.connect(cwd, force);
                        completed.push((name, connection, result));
                    }
                    Ok::<_, String>(completed)
                })
            })
            .collect::<Vec<_>>();
        let mut completed = Vec::new();
        for handle in handles {
            completed.extend(
                handle
                    .join()
                    .map_err(|_| "MCP startup worker panicked".to_string())??,
            );
        }
        Ok(completed)
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
                let alive = connection
                    .client
                    .as_mut()
                    .map(McpClient::is_alive)
                    .unwrap_or(false);
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
        status.insert(name.clone(), match result { Ok(()) => json!({"state": "ready", "tools": connection.tools.get("tools").and_then(Value::as_array).map_or(0, Vec::len)}), Err(error) => json!({"state": connection.state.as_str(), "error": error}) });
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
        connection.failures = 0;
        connection.retry_after = None;
        connection.connect(cwd, true)?;
        crate::capability::invalidate();
        Ok(
            json!({"server": server_name, "state": connection.state.as_str(), "tools": connection.tools.get("tools").and_then(Value::as_array).map_or(0, Vec::len)}),
        )
    })
}
