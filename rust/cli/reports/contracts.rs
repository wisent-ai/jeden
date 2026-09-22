//! `jeden contracts`: the task and communication contracts as one text, and
//! that text installed into another harness's system prompt.
//!
//! Jeden's own sessions get the contracts from the binary. Sessions run by
//! Omp get them from `~/.omp/agent/APPEND_SYSTEM.md`, which Omp appends to
//! every system prompt verbatim. `install` writes the rendered text there
//! between two marker lines, replacing the previous block and leaving the
//! rest of the file alone; `status` says whether the installed block matches
//! what the binary would render now.

use serde_json::json;
use std::fs;
use std::path::{Path, PathBuf};

use crate::agent::{communication_contract, task_contract};
use crate::cli::config::ui_language;
use crate::Args;

const BLOCK_START: &str = "<!-- jeden contracts: start -->";
const BLOCK_END: &str = "<!-- jeden contracts: end -->";
const USAGE: &str =
    "Usage: jeden contracts [render|status|install] [--omp|--file <path>] [--json] [--cwd path]";

/// The text Jeden puts into every system prompt: the task contract and the
/// communication contract in force, in the conversation language.
pub(crate) fn render(cwd: &Path) -> String {
    let config = crate::load_config(cwd);
    let language = ui_language(&config);
    let (source, communication) =
        communication_contract::resolve(&config.contracts.communication, &language);
    let mut text = task_contract::section(&language);
    if !communication.is_empty() {
        text.push('\n');
        text.push_str(match source {
            communication_contract::Source::Default => "Communication contract (Jeden default):\n",
            _ => "Communication contract:\n",
        });
        text.push_str(communication);
        text.push('\n');
    }
    let functionality = config.contracts.functionality.trim();
    if !functionality.is_empty() {
        text.push_str("\nFunctionality contract:\n");
        text.push_str(functionality);
        text.push('\n');
    }
    text
}

fn block(rendered: &str) -> String {
    format!("{BLOCK_START}\n{}\n{BLOCK_END}", rendered.trim_end())
}

/// The installed block of `file`, if it carries one.
fn installed_block(file: &Path) -> Option<String> {
    let text = fs::read_to_string(file).ok()?;
    let start = text.find(BLOCK_START)?;
    let end = text[start..].find(BLOCK_END)? + start + BLOCK_END.len();
    Some(text[start..end].to_string())
}

/// `file` with its Jeden block replaced by `block`, or appended when absent.
fn spliced(existing: &str, block: &str) -> String {
    if let Some(start) = existing.find(BLOCK_START) {
        if let Some(end) = existing[start..].find(BLOCK_END) {
            let end = start + end + BLOCK_END.len();
            return format!("{}{block}{}", &existing[..start], &existing[end..]);
        }
    }
    let mut out = existing.trim_end().to_string();
    if !out.is_empty() {
        out.push_str("\n\n");
    }
    out.push_str(block);
    out.push('\n');
    out
}

fn omp_append_system_file() -> Result<PathBuf, String> {
    let home = std::env::var_os("HOME").ok_or("HOME is not set")?;
    Ok(PathBuf::from(home).join(".omp/agent/APPEND_SYSTEM.md"))
}

struct Target {
    name: &'static str,
    file: PathBuf,
}

fn target(rest: &[String]) -> Result<Target, String> {
    let mut iter = rest.iter();
    let mut target = None;
    while let Some(token) = iter.next() {
        match token.as_str() {
            "--omp" => {
                target = Some(Target {
                    name: "omp",
                    file: omp_append_system_file()?,
                })
            }
            "--file" => {
                let path = iter.next().ok_or("--file requires a path")?;
                target = Some(Target {
                    name: "file",
                    file: PathBuf::from(path),
                })
            }
            other => return Err(format!("unknown contracts option: {other}\n{USAGE}")),
        }
    }
    target.ok_or_else(|| {
        format!("contracts install and status require --omp or --file <path>\n{USAGE}")
    })
}

pub(crate) fn command(args: &Args) -> Result<String, String> {
    let (verb, rest) = args
        .positionals
        .split_first()
        .map(|(verb, rest)| (verb.as_str(), rest))
        .unwrap_or(("render", &[]));
    match verb {
        "render" => {
            if !rest.is_empty() {
                return Err(USAGE.into());
            }
            let rendered = render(&args.cwd);
            Ok(if args.json {
                serde_json::to_string_pretty(&json!({"text": rendered}))
                    .map_err(|error| error.to_string())?
                    + "\n"
            } else {
                rendered
            })
        }
        "status" => {
            let target = target(rest)?;
            let rendered = block(&render(&args.cwd));
            let installed = installed_block(&target.file);
            let state = match &installed {
                Some(installed) if installed.trim() == rendered.trim() => "current",
                Some(_) => "stale",
                None => "absent",
            };
            let path = target.file.display().to_string();
            if args.json {
                return Ok(serde_json::to_string_pretty(&json!({
                    "target": target.name,
                    "path": path,
                    "state": state,
                }))
                .map_err(|error| error.to_string())?
                    + "\n");
            }
            let text = match state {
                "current" => format!("current: {path} carries the contracts this binary renders\n"),
                "stale" => format!(
                    "stale: {path} carries older contracts; run jeden contracts install --{}\n",
                    target.name
                ),
                _ => format!(
                    "absent: {path} carries no Jeden contracts; run jeden contracts install --{}\n",
                    target.name
                ),
            };
            if state == "current" {
                Ok(text)
            } else {
                Err(text.trim_end().to_string())
            }
        }
        "install" => {
            let target = target(rest)?;
            let rendered = block(&render(&args.cwd));
            let existing = fs::read_to_string(&target.file).unwrap_or_default();
            let next = spliced(&existing, &rendered);
            let changed = next != existing;
            if changed {
                if let Some(parent) = target.file.parent() {
                    fs::create_dir_all(parent)
                        .map_err(|error| format!("cannot create {}: {error}", parent.display()))?;
                }
                crate::cli::config::migrations::write_text_atomic(&target.file, &next)?;
            }
            let path = target.file.display().to_string();
            Ok(if args.json {
                serde_json::to_string_pretty(&json!({
                    "target": target.name,
                    "path": path,
                    "changed": changed,
                }))
                .map_err(|error| error.to_string())?
                    + "\n"
            } else if changed {
                format!("Installed the Jeden contracts into {path}\n")
            } else {
                format!("{path} already carries the Jeden contracts\n")
            })
        }
        _ => Err(USAGE.into()),
    }
}
