//! Finding a language server that is actually installed, and picking the
//! right one for the file being asked about.
//!
//! Split out of `tool_runtime/language/lsp.rs`, which had grown past the
//! module line cap.

use crate::tool_runtime::shared::string_input;
use serde_json::Value;
use std::env;
use std::path::Path;
use std::process::{Command, Stdio};

pub(super) fn executable_exists(name: &str) -> bool {
    if name.contains(std::path::MAIN_SEPARATOR) {
        return Path::new(name).is_file();
    }
    env::var_os("PATH")
        .map(|paths| env::split_paths(&paths).any(|path| path.join(name).is_file()))
        .unwrap_or(false)
}
/// Whether a language server on `PATH` answers `--version`. The program's own
/// exit is the verdict; a slow machine is not a missing server.
fn probe_server(name: &str) -> bool {
    if !executable_exists(name) {
        return false;
    }
    let Ok(mut child) = Command::new(name)
        .arg("--version")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
    else {
        return false;
    };
    match child.wait() {
        Ok(status) => status.success(),
        Err(_) => false,
    }
}

pub(crate) fn healthy_servers() -> Vec<String> {
    [
        "rust-analyzer",
        "pyright-langserver",
        "typescript-language-server",
    ]
    .into_iter()
    .filter(|name| probe_server(name))
    .map(ToOwned::to_owned)
    .collect()
}

fn inferred_server(path: &Path) -> Option<(String, Vec<String>)> {
    match path
        .extension()
        .and_then(|value| value.to_str())
        .unwrap_or("")
    {
        "rs" if executable_exists("rust-analyzer") => Some(("rust-analyzer".into(), Vec::new())),
        "py" if executable_exists("pyright-langserver") => {
            Some(("pyright-langserver".into(), vec!["--stdio".into()]))
        }
        "js" | "jsx" | "ts" | "tsx" if executable_exists("typescript-language-server") => {
            Some(("typescript-language-server".into(), vec!["--stdio".into()]))
        }
        _ => None,
    }
}

pub(super) fn command_for(input: &Value, path: &Path) -> Result<(String, Vec<String>), String> {
    if let Some(program) = string_input(input, "server") {
        let args = input
            .get("serverArgs")
            .and_then(Value::as_array)
            .map(|items| {
                items
                    .iter()
                    .filter_map(Value::as_str)
                    .map(ToOwned::to_owned)
                    .collect()
            })
            .unwrap_or_default();
        return Ok((program, args));
    }
    inferred_server(path)
        .ok_or_else(|| format!("no healthy LSP server discovered for {}", path.display()))
}

pub(super) fn language_id(path: &Path) -> &'static str {
    match path
        .extension()
        .and_then(|value| value.to_str())
        .unwrap_or("")
    {
        "rs" => "rust",
        "py" => "python",
        "ts" => "typescript",
        "tsx" => "typescriptreact",
        "jsx" => "javascriptreact",
        _ => "javascript",
    }
}
