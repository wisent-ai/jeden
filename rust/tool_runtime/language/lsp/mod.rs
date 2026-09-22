//! The language tools: ask an installed language server about a file.

use serde_json::{json, Value};
use std::collections::BTreeMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{LazyLock, Mutex};

use crate::tool_runtime::shared::{jail_path, string_input, u64_input};
use crate::tool_runtime::ToolRuntime;

mod client;
mod discovery;

use client::{await_response, ensure_open, file_uri, send, start, LspClient};
use discovery::command_for;
pub(super) use discovery::healthy_servers;

const MAX_LSP_SERVERS: usize = 8;
static NEXT_REQUEST: AtomicU64 = AtomicU64::new(1);
static SERVERS: LazyLock<Mutex<BTreeMap<String, LspClient>>> =
    LazyLock::new(|| Mutex::new(BTreeMap::new()));

pub(crate) fn lsp(runtime: &ToolRuntime<'_>, input: &Value) -> Result<Value, String> {
    let action = string_input(input, "action").unwrap_or_else(|| "health".into());
    if action == "health" {
        let healthy = healthy_servers();
        let servers = [
            "rust-analyzer",
            "pyright-langserver",
            "typescript-language-server",
        ]
        .into_iter()
        .map(|name| json!({"name":name,"healthy":healthy.iter().any(|item|item==name)}))
        .collect::<Vec<_>>();
        return Ok(
            json!({"ok":true,"status":if healthy.is_empty(){"unavailable"}else{"healthy"},"servers":servers,"maxServers":MAX_LSP_SERVERS}),
        );
    }
    let label = string_input(input, "path").ok_or("LSP action requires path")?;
    let path = jail_path(runtime.cwd, &label)?;
    let (program, args) = command_for(input, &path)?;
    let key = format!(
        "{}\0{}\0{}",
        runtime.cwd.display(),
        program,
        args.join("\0")
    );
    let mut servers = SERVERS.lock().map_err(|_| "LSP manager poisoned")?;
    if !servers.contains_key(&key) {
        if servers.len() >= MAX_LSP_SERVERS {
            return Err(format!("LSP server limit reached ({MAX_LSP_SERVERS})"));
        }
        servers.insert(key.clone(), start(runtime, &program, &args)?);
    }
    let client = servers.get_mut(&key).ok_or("LSP server unavailable")?;
    if let Some(status) = client.child.try_wait().map_err(|error| error.to_string())? {
        return Err(format!("LSP server exited: {status}"));
    }
    ensure_open(client, &path)?;
    let uri = file_uri(&path)?;
    let position = json!({"line":u64_input(input,"line",1).saturating_sub(1),"character":u64_input(input,"column",1).saturating_sub(1)});
    let (method, params) = match action.as_str() {
        "diagnostics" => (
            "textDocument/diagnostic",
            json!({"textDocument":{"uri":uri}}),
        ),
        "definition" => (
            "textDocument/definition",
            json!({"textDocument":{"uri":uri},"position":position}),
        ),
        "references" => (
            "textDocument/references",
            json!({"textDocument":{"uri":uri},"position":position,"context":{"includeDeclaration":true}}),
        ),
        "rename" => (
            "textDocument/rename",
            json!({"textDocument":{"uri":uri},"position":position,"newName":string_input(input,"newName").ok_or("LSP rename requires newName")?}),
        ),
        "codeActions" => (
            "textDocument/codeAction",
            json!({"textDocument":{"uri":uri},"range":{"start":position,"end":position},"context":{"diagnostics":input.get("diagnostics").cloned().unwrap_or_else(||json!([]))}}),
        ),
        "format" => (
            "textDocument/formatting",
            json!({"textDocument":{"uri":uri},"options":{"tabSize":u64_input(input,"tabSize",4),"insertSpaces":input.get("insertSpaces").and_then(Value::as_bool).unwrap_or(true)}}),
        ),
        other => return Err(format!("unsupported LSP action: {other}")),
    };
    let id = NEXT_REQUEST.fetch_add(1, Ordering::Relaxed);
    send(
        &mut client.stdin,
        &json!({"jsonrpc":"2.0","id":id,"method":method,"params":params}),
    )?;
    let result = await_response(runtime, client, id)?;
    Ok(json!({"ok":true,"action":action,"path":label,"server":program,"result":result}))
}
