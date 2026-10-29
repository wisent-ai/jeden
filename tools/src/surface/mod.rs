//! Jeden's public surface: the command vocabulary the binary answers to.
//!
//! Jeden is not a library anyone links against. What a caller depends on is
//! which commands the binary answers to: the `jeden <command>` subcommands
//! scripts and the ACP and editor clients invoke, and the `/command` names an
//! interactive user types. Adding one is a capability; removing one breaks
//! whoever scripted it yesterday. Two families, namespaced because a CLI
//! subcommand and a slash command of the same word are different promises:
//!
//!   cli:<name>      a command the binary answers to: a variant of the
//!                   `Command` enum in rust/cli/invocation/command.rs, which
//!                   the parser and the dispatcher both read, spelled as its
//!                   `#[serde(rename_all = "kebab-case")]` spells it;
//!   slash:/<name>   a builtin slash command or alias, read from the catalogue
//!                   the binary compiles in.
//!
//! Options are excluded: a flag modifies a command, and Jeden's own
//! capability model has no kind for it. Everything is read statically, never
//! by building, so the surface of a published source revision can be
//! recovered exactly. A file that does not parse, or a declaration site that
//! has moved, is refused; it never degrades to a smaller surface, which would
//! read as a clean removal and mislabel the release.

mod source;

use serde_json::{json, Value};
use source::Source;
use std::collections::BTreeSet;
use std::fs;
use std::path::Path;

/// The one declaration of every top-level command.
const COMMAND_FILE: &str = "rust/cli/invocation/command.rs";
/// The serde spelling the reader applies to the enum's variants.
const COMMAND_SPELLING: &str = "kebab-case";
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

/// Every command name the binary answers to: each variant of the `Command`
/// enum, spelled as serde's `kebab-case` spells it on the command line.
fn cli_commands(root: &Path) -> Result<BTreeSet<String>, String> {
    let source = Source::read(&root.join(COMMAND_FILE), COMMAND_FILE)?;
    let declaration = source.sole_anchor(r"\benum\s+Command\s*\{", "Command enum")?;
    let rename = source.sole_anchor(
        r#"#\s*\[\s*serde\s*\(\s*rename_all\s*=\s*""#,
        "serde rename_all",
    )?;
    if rename.start() > declaration.start() {
        return Err(format!(
            "{COMMAND_FILE}: the serde rename_all attribute follows the Command enum instead of declaring its spelling"
        ));
    }
    let spelling = source.literal_at(rename.end() - 1)?;
    if spelling != COMMAND_SPELLING {
        return Err(format!(
            "{COMMAND_FILE}: the Command enum is spelled {spelling:?}; the surface reader spells {COMMAND_SPELLING:?} only"
        ));
    }
    let block = declaration.end() - 1;
    let end = source.balanced_end(block, b'{', b'}')?;
    let mut names = BTreeSet::new();
    for variant in source.variants(block + 1, end - 1)? {
        names.insert(kebab_case(&variant));
    }
    if names.is_empty() {
        return Err(format!(
            "{COMMAND_FILE}: the Command enum declares no variant"
        ));
    }
    Ok(names)
}

/// `variant` as serde's `kebab-case` spells it: a hyphen before every
/// uppercase letter but the first, and every letter lowercase.
fn kebab_case(variant: &str) -> String {
    let mut spelled = String::with_capacity(variant.len() + variant.len() / 2);
    for (index, letter) in variant.char_indices() {
        if index > 0 && letter.is_uppercase() {
            spelled.push('-');
        }
        spelled.extend(letter.to_lowercase());
    }
    spelled
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
