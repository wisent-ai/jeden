//! Starting one external tool server, by process or by URL, and refusing a
//! configuration that mixes the two.
//!
//! Split out of `mcp/client.rs`, which had grown past the module line cap.

use super::framing::{drain_stderr, read_messages};
use super::super::servers::{resolve_server_cwd, string_field};
use super::{HttpTransport, McpClient, StdioTransport};
use reqwest::blocking::Client as HttpClient;
use serde_json::Value;
use std::path::Path;
use std::process::{Command, Stdio};

impl McpClient {
    pub(super) fn start_stdio(server: &Value, cwd: &Path) -> Result<StdioTransport, String> {
        if server.get("url").is_some() {
            return Err("stdio MCP config must not contain url".into());
        }
        let command = string_field(server, "command")
            .filter(|command| !command.is_empty())
            .ok_or("stdio MCP server.command must be a non-empty string")?;
        let args = match server.get("args") {
            None => Vec::new(),
            Some(Value::Array(values)) => values
                .iter()
                .map(|value| {
                    value
                        .as_str()
                        .map(ToOwned::to_owned)
                        .ok_or_else(|| "stdio MCP server.args entries must be strings".to_string())
                })
                .collect::<Result<Vec<_>, _>>()?,
            Some(_) => return Err("stdio MCP server.args must be an array".into()),
        };
        let mut builder = Command::new(command);
        builder.env_clear();
        builder
            .args(args)
            .current_dir(resolve_server_cwd(cwd, server))
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        match server.get("env") {
            None => {}
            Some(Value::Object(values)) => {
                for (key, value) in values {
                    match value {
                        Value::Null => {
                            builder.env_remove(key);
                        }
                        Value::String(value) => {
                            builder.env(key, value);
                        }
                        _ => {
                            return Err("stdio MCP server.env values must be strings or null".into())
                        }
                    }
                }
            }
            Some(_) => return Err("stdio MCP server.env must be an object".into()),
        }
        let mut child = builder
            .spawn()
            .map_err(|error| format!("failed to start MCP server: {error}"))?;
        let stdout = child.stdout.take().ok_or("MCP server stdout unavailable")?;
        let stderr = child.stderr.take().ok_or("MCP server stderr unavailable")?;
        Ok(StdioTransport {
            child,
            responses: read_messages(stdout),
            stderr: drain_stderr(stderr),
        })
    }

    pub(super) fn start_http(server: &Value) -> Result<HttpTransport, String> {
        if server.get("command").is_some() || server.get("args").is_some() {
            return Err("streamable HTTP MCP config must not contain command or args".into());
        }
        let url = string_field(server, "url")
            .filter(|url| url.starts_with("http://") || url.starts_with("https://"))
            .ok_or("streamable HTTP MCP server.url must be an http(s) URL")?
            .to_string();
        let client = crate::net::blocking_builder()
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .map_err(|error| format!("failed to create MCP HTTP client: {error}"))?;
        Ok(HttpTransport {
            client,
            url,
            session_id: None,
        })
    }
}
