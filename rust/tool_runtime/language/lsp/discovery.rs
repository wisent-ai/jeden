//! Finding a language server that is actually installed, and picking the
//! right one for the file being asked about.

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
        .is_some_and(|paths| env::split_paths(&paths).any(|path| path.join(name).is_file()))
}
/// Whether a language server on `PATH` answers `--version`. The program's own
/// exit is the verdict; a slow machine is not a missing server. A probe that
/// cannot be started or waited for counts as no answer and says why.
fn probe_server(name: &str) -> bool {
    if !executable_exists(name) {
        return false;
    }
    let mut child = match Command::new(name)
        .arg("--version")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
    {
        Ok(child) => child,
        Err(error) => {
            eprintln!("jeden: language server {name} could not be started for its probe: {error}");
            return false;
        }
    };
    match stado_wait::blocking(stado_wait::Kind::Process, format!("{name} --version"), name, || child.wait()) {
        Ok(status) => status.success(),
        Err(error) => {
            eprintln!("jeden: the probe of language server {name} could not be waited for: {error}");
            false
        }
    }
}

/// The languages Jeden starts a server for: read from a file's extension
/// and spelled in `didOpen` by their LSP languageId
/// (https://microsoft.github.io/language-server-protocol/, TextDocumentItem).
#[derive(Clone, Copy, serde::Serialize, serde::Deserialize)]
enum Language {
    #[serde(rename(serialize = "rust", deserialize = "rs"))]
    Rust,
    #[serde(rename(serialize = "python", deserialize = "py"))]
    Python,
    #[serde(rename(serialize = "typescript", deserialize = "ts"))]
    Typescript,
    #[serde(rename(serialize = "typescriptreact", deserialize = "tsx"))]
    TypescriptReact,
    #[serde(rename(serialize = "javascript", deserialize = "js"))]
    Javascript,
    #[serde(rename(serialize = "javascriptreact", deserialize = "jsx"))]
    JavascriptReact,
}

impl Language {
    const ALL: &'static [Language] = &[
        Language::Rust,
        Language::Python,
        Language::Typescript,
        Language::TypescriptReact,
        Language::Javascript,
        Language::JavascriptReact,
    ];

    /// The language of `path`, named by its extension; none for a file in
    /// a language Jeden starts no server for, which is all the parse of the
    /// extension can refuse.
    fn of(path: &Path) -> Option<Self> {
        let extension = path.extension()?.to_str()?;
        match serde_json::from_value(Value::String(extension.to_owned())) {
            Ok(language) => Some(language),
            Err(_another_language) => None,
        }
    }

    /// The server program that reads this language and the arguments it
    /// is started with.
    fn server(self) -> (&'static str, &'static [&'static str]) {
        match self {
            Language::Rust => ("rust-analyzer", &[]),
            Language::Python => ("pyright-langserver", &["--stdio"]),
            Language::Typescript
            | Language::TypescriptReact
            | Language::Javascript
            | Language::JavascriptReact => ("typescript-language-server", &["--stdio"]),
        }
    }
}

/// Every server some declared language uses that answers its probe.
pub(crate) fn healthy_servers() -> Vec<String> {
    let mut names: Vec<&str> = Vec::new();
    for language in Language::ALL {
        let (program, _) = language.server();
        if !names.contains(&program) {
            names.push(program);
        }
    }
    names
        .into_iter()
        .filter(|name| probe_server(name))
        .map(ToOwned::to_owned)
        .collect()
}

fn inferred_server(path: &Path) -> Option<(String, Vec<String>)> {
    let (program, args) = Language::of(path)?.server();
    executable_exists(program).then(|| {
        (
            program.to_owned(),
            args.iter().map(|arg| (*arg).to_owned()).collect(),
        )
    })
}

pub(super) fn command_for(input: &Value, path: &Path) -> Result<(String, Vec<String>), String> {
    if let Some(program) = string_input(input, "server") {
        // No serverArgs is a server started without arguments; an argument
        // that is not text is refused rather than dropped from the command.
        let args = match input.get("serverArgs") {
            None => Vec::new(),
            Some(Value::Array(items)) => items
                .iter()
                .map(|item| {
                    item.as_str()
                        .map(ToOwned::to_owned)
                        .ok_or_else(|| format!("serverArgs holds {item}, which is not text"))
                })
                .collect::<Result<Vec<_>, _>>()?,
            Some(other) => return Err(format!("serverArgs is {other}, not a list of arguments")),
        };
        return Ok((program, args));
    }
    inferred_server(path)
        .ok_or_else(|| format!("no healthy LSP server discovered for {}", path.display()))
}

/// The LSP languageId of `path`, or why there is none: its extension names
/// no language Jeden starts a server for.
pub(super) fn language_id(path: &Path) -> Result<String, String> {
    let language = Language::of(path).ok_or_else(|| {
        format!(
            "{} names no language Jeden starts a language server for",
            path.display()
        )
    })?;
    match serde_json::to_value(language) {
        Ok(Value::String(id)) => Ok(id),
        other => unreachable!("a unit language serializes as its languageId, not {other:?}"),
    }
}
