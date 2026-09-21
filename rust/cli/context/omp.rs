//! Installing the advisor into another harness.
//!
//! Omp is not ours to patch, and it does not need to be: it loads custom
//! tools from `~/.omp/agent/tools/*.ts`, the same documented directory
//! `show_user` already uses. `jeden context install --omp` renders that file
//! from the binary, so an Omp session calls the same advisor a Jeden session
//! does and neither carries a second implementation.

use std::fs;
use std::path::{Path, PathBuf};

const TEMPLATE: &str = include_str!("jeden_context_tool.ts");
const BINARY_PLACEHOLDER: &str = "__JEDEN_BIN__";
pub(super) const TOOL_FILE: &str = "jeden_context.ts";

pub(super) struct Target {
    pub(super) name: &'static str,
    pub(super) file: PathBuf,
}

/// `--omp` resolves Omp's own tool directory; `--file` writes anywhere, which
/// is how a test drives this without touching the operator's harness.
pub(super) fn target(rest: &[String]) -> Result<Target, String> {
    let mut iter = rest.iter();
    let mut target = None;
    while let Some(token) = iter.next() {
        match token.as_str() {
            "--omp" => {
                target = Some(Target {
                    name: "omp",
                    file: omp_tool_file()?,
                })
            }
            "--file" => {
                let path = iter.next().ok_or("--file requires a path")?;
                target = Some(Target {
                    name: "file",
                    file: PathBuf::from(path),
                })
            }
            other => return Err(format!("unknown context option: {other}")),
        }
    }
    target.ok_or_else(|| {
        "context install and context installed require --omp or --file <path>".to_string()
    })
}

fn omp_tool_file() -> Result<PathBuf, String> {
    let home = std::env::var_os("HOME").ok_or("HOME is not set")?;
    Ok(PathBuf::from(home).join(".omp/agent/tools").join(TOOL_FILE))
}

/// The tool source this binary would install, with its own absolute path
/// bound in: an Omp session must reach the same Jeden that rendered it.
pub(super) fn rendered() -> Result<String, String> {
    let binary = std::env::current_exe()
        .map_err(|error| format!("the running jeden binary cannot be located: {error}"))?;
    Ok(TEMPLATE.replace(BINARY_PLACEHOLDER, &binary.display().to_string()))
}

pub(super) fn state(file: &Path, rendered: &str) -> &'static str {
    match fs::read_to_string(file) {
        Ok(existing) if existing == rendered => "current",
        Ok(_) => "stale",
        Err(_) => "absent",
    }
}

pub(super) fn install(file: &Path, rendered: &str) -> Result<bool, String> {
    if state(file, rendered) == "current" {
        return Ok(false);
    }
    if let Some(parent) = file.parent() {
        fs::create_dir_all(parent)
            .map_err(|error| format!("cannot create {}: {error}", parent.display()))?;
    }
    crate::cli::config::migrations::write_text_atomic(file, rendered)?;
    Ok(true)
}
