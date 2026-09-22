//! Jeden's public surface: the command vocabulary the binary answers to.
//!
//! Jeden is not a library anyone links against. What a caller depends on is
//! which commands the binary answers to: the `jeden <command>` subcommands
//! scripts and the ACP and editor clients invoke, and the `/command` names an
//! interactive user types. Adding one is a capability; removing one breaks
//! whoever scripted it yesterday. Two families, namespaced because a CLI
//! subcommand and a slash command of the same word are different promises:
//!
//!   cli:<name>      a command the binary dispatches, read from the
//!                   dispatcher in rust/main.rs (its match arms, the
//!                   `command == "..."` checks ahead of it, and the
//!                   `matches!(command.as_str(), ...)` pre-dispatch);
//!   slash:/<name>   a builtin slash command or alias, read from the catalogue
//!                   the binary compiles in.
//!
//! Options are excluded: a flag modifies a command, and Jeden's own
//! capability model has no kind for it. Everything is read statically, never
//! by building, so the surface of a published source revision can be
//! recovered exactly. A file that does not parse, or a declaration site that
//! has moved, is refused; it never degrades to a smaller surface, which would
//! read as a clean removal and mislabel the release.

mod arms;
mod source;

use arms::match_arm_patterns;
use serde_json::{json, Value};
use source::Source;
use std::collections::BTreeSet;
use std::fs;
use std::path::Path;

const DISPATCH_FILE: &str = "rust/main.rs";
/// The builtin slash commands are a declaration the binary compiles in with
/// `include_str!` (rust/capability/builtin/mod.rs), not Rust source.
const REGISTRY_FILE: &str = "rust/capability/builtin/builtin-slash-commands.json";

pub(crate) fn run(arguments: &[String]) -> Result<u8, String> {
    let root = match arguments {
        [] => crate::repository_root(),
        [root] => root.into(),
        _ => return Err("usage: jeden-tools surface [ROOT]".into()),
    };
    let names = surface(&root)?;
    let document = serde_json::to_string_pretty(&json!({ "surface": names }))
        .map_err(|error| error.to_string())?;
    println!("{document}");
    Ok(0)
}

pub(crate) fn surface(root: &Path) -> Result<Vec<String>, String> {
    let mut names = cli_commands(root)?
        .into_iter()
        .map(|name| format!("cli:{name}"))
        .collect::<BTreeSet<_>>();
    names.extend(
        slash_commands(root)?
            .into_iter()
            .map(|name| format!("slash:/{name}")),
    );
    Ok(names.into_iter().collect())
}

/// Every command name the binary dispatches, from rust/main.rs.
fn cli_commands(root: &Path) -> Result<BTreeSet<String>, String> {
    let source = Source::read(&root.join(DISPATCH_FILE), DISPATCH_FILE)?;
    let anchor = source.sole_anchor(
        r"match\s+args\.command\.as_str\(\)\s*\{",
        "command dispatcher",
    )?;
    let block = anchor.end() - 1;
    let end = source.balanced_end(block, b'{', b'}')?;
    let mut names = match_arm_patterns(&source, block + 1, end)?;
    for equality in source.anchors(r#"\bcommand\s*==\s*""#)? {
        names.push(source.literal_at(equality.end() - 1)?);
    }
    for guard in source.anchors(r"matches!\s*\(\s*(?:args\.)?command\.as_str\(\)\s*,\s*")? {
        let opener = guard.start()
            + guard
                .as_str()
                .find('(')
                .ok_or("a matches! guard has no parenthesis")?;
        let close = source.balanced_end(opener, b'(', b')')?;
        names.extend(source.literals_within(guard.end(), close));
    }
    let kept = names
        .into_iter()
        .filter(|name| !name.is_empty() && !name.starts_with('-'))
        .collect::<BTreeSet<_>>();
    if kept.is_empty() {
        return Err(format!(
            "{DISPATCH_FILE}: dispatcher yielded no command names"
        ));
    }
    Ok(kept)
}

/// Every builtin slash command and alias, from the catalogue the binary
/// compiles in.
fn slash_commands(root: &Path) -> Result<BTreeSet<String>, String> {
    let text = fs::read_to_string(root.join(REGISTRY_FILE))
        .map_err(|error| format!("{REGISTRY_FILE}: cannot read the slash catalogue: {error}"))?;
    let catalogue: Value = serde_json::from_str(&text)
        .map_err(|error| format!("{REGISTRY_FILE}: cannot read the slash catalogue: {error}"))?;
    let entries = catalogue
        .as_array()
        .ok_or_else(|| format!("{REGISTRY_FILE}: the slash catalogue is not a list"))?;
    let mut names = BTreeSet::new();
    for entry in entries {
        let name = entry["name"]
            .as_str()
            .ok_or_else(|| format!("{REGISTRY_FILE}: an entry carries no name: {entry}"))?;
        names.insert(name.to_string());
        let aliases = match &entry["aliases"] {
            Value::Null => Vec::new(),
            Value::Array(aliases) => aliases
                .iter()
                .map(|alias| alias.as_str().map(str::to_string))
                .collect::<Option<Vec<_>>>()
                .ok_or_else(|| format!("{REGISTRY_FILE}: {name} carries malformed aliases"))?,
            _ => return Err(format!("{REGISTRY_FILE}: {name} carries malformed aliases")),
        };
        names.extend(aliases);
    }
    names.retain(|name| !name.is_empty());
    if names.is_empty() {
        return Err(format!("{REGISTRY_FILE}: slash registry yielded no names"));
    }
    Ok(names)
}
