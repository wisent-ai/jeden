//! `jeden contracts`: the task and communication contracts as one text, and
//! that text installed into another harness's system prompt file.
//!
//! Jeden's own sessions get the contracts from the binary. `install` writes
//! the rendered text into the file `--file` names, between two marker lines,
//! replacing the previous block and leaving the rest of the file alone;
//! `status` says whether the installed block matches what the binary would
//! render now.

use serde_json::json;
use std::fs;
use std::path::{Path, PathBuf};

use crate::agent::{communication_contract, task_contract};
use crate::cli::config::ui_language;
use crate::Args;

const BLOCK_START: &str = "<!-- jeden contracts: start -->";
const BLOCK_END: &str = "<!-- jeden contracts: end -->";
const USAGE: &str =
    "Usage: jeden contracts [render|status|install|uninstall] [--file <path>] [--json] [--cwd path]";

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

/// `existing` without its Jeden block and the blank line `spliced` put before
/// it; `None` when it carries no block.
fn unspliced(existing: &str) -> Option<String> {
    let start = existing.find(BLOCK_START)?;
    let end = start + existing[start..].find(BLOCK_END)? + BLOCK_END.len();
    let before = existing[..start].trim_end();
    let after = existing[end..].trim_start_matches('\n');
    let mut out = before.to_string();
    if !before.is_empty() && !after.is_empty() {
        out.push_str("\n\n");
    }
    out.push_str(after);
    if !out.is_empty() && !out.ends_with('\n') {
        out.push('\n');
    }
    Some(out)
}

/// The file `--file` names, the only target there is.
fn target(rest: &[String]) -> Result<PathBuf, String> {
    let mut iter = rest.iter();
    let mut target = None;
    while let Some(token) = iter.next() {
        match token.as_str() {
            "--file" => {
                let path = iter.next().ok_or_else(|| {
                    crate::cli::invocation::refusal::usage("--file requires a path")
                })?;
                target = Some(PathBuf::from(path))
            }
            other => {
                return Err(crate::cli::invocation::refusal::usage(format!(
                    "unknown contracts option: {other}\n{USAGE}"
                )))
            }
        }
    }
    target.ok_or_else(|| {
        crate::cli::invocation::refusal::usage(format!(
            "contracts install, uninstall and status require --file <path>\n{USAGE}"
        ))
    })
}

/// The file's text, or nothing when it does not exist yet; any other read
/// failure names the file.
fn existing_text(file: &Path) -> Result<String, String> {
    match fs::read_to_string(file) {
        Ok(text) => Ok(text),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(String::new()),
        Err(error) => Err(format!("cannot read {}: {error}", file.display())),
    }
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
                return Err(crate::cli::invocation::refusal::usage(USAGE));
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
            let file = target(rest)?;
            let rendered = block(&render(&args.cwd));
            let installed = installed_block(&file);
            let state = match &installed {
                Some(installed) if installed.trim() == rendered.trim() => "current",
                Some(_) => "stale",
                None => "absent",
            };
            let path = file.display().to_string();
            if args.json {
                return Ok(serde_json::to_string_pretty(&json!({
                    "path": path,
                    "state": state,
                }))
                .map_err(|error| error.to_string())?
                    + "\n");
            }
            let text = match state {
                "current" => format!("current: {path} carries the contracts this binary renders\n"),
                "stale" => format!(
                    "stale: {path} carries older contracts; run jeden contracts install --file {path}\n"
                ),
                _ => format!(
                    "absent: {path} carries no Jeden contracts; run jeden contracts install --file {path}\n"
                ),
            };
            if state == "current" {
                Ok(text)
            } else {
                Err(text.trim_end().to_string())
            }
        }
        "install" => {
            let file = target(rest)?;
            let rendered = block(&render(&args.cwd));
            let existing = existing_text(&file)?;
            let next = spliced(&existing, &rendered);
            let changed = next != existing;
            if changed {
                if let Some(parent) = file.parent() {
                    fs::create_dir_all(parent)
                        .map_err(|error| format!("cannot create {}: {error}", parent.display()))?;
                }
                crate::cli::config::migrations::write_text_atomic(&file, &next)?;
            }
            let path = file.display().to_string();
            Ok(if args.json {
                serde_json::to_string_pretty(&json!({
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
        // The inverse of install (cli.md rule 2): the block goes, the rest of
        // the file stays as it was.
        "uninstall" => {
            let file = target(rest)?;
            let existing = existing_text(&file)?;
            let next = unspliced(&existing);
            if let Some(next) = &next {
                crate::cli::config::migrations::write_text_atomic(&file, next)?;
            }
            let path = file.display().to_string();
            Ok(if args.json {
                serde_json::to_string_pretty(&json!({
                    "path": path,
                    "changed": next.is_some(),
                }))
                .map_err(|error| error.to_string())?
                    + "\n"
            } else if next.is_some() {
                format!("Removed the Jeden contracts from {path}\n")
            } else {
                format!("{path} carries no Jeden contracts\n")
            })
        }
        _ => Err(crate::cli::invocation::refusal::usage(USAGE)),
    }
}
