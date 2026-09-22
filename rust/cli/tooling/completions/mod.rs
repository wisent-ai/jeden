//! Shell completion script generation.
//!
//! Everything is derived from two in-repo metadata tables so future commands
//! come free: the `jeden --help` usage text builder (`crate::usage`, the CLI
//! usage surface) and the builtin slash-command registry
//! (`capability::builtin_slash_specs`, including aliases). No shell script is
//! hand-written per command.

use crate::Args;

#[derive(Clone, Debug)]
pub(super) struct FlagSpec {
    /// Long form, e.g. `--cwd`. Short flags keep their single dash.
    pub(super) name: String,
    pub(super) takes_value: bool,
}

#[derive(Clone, Debug, Default)]
pub(super) struct CommandSpec {
    pub(super) name: String,
    pub(super) description: String,
    pub(super) flags: Vec<FlagSpec>,
    /// Sub-action words parsed from the first `<a|b|c>` / `[a|b|c]` group.
    pub(super) actions: Vec<String>,
}

#[derive(Clone, Debug)]
pub(super) struct SlashEntry {
    /// `/name` or `/alias`.
    pub(super) word: String,
    pub(super) description: String,
}

#[derive(Clone, Debug, Default)]
pub(super) struct CompletionModel {
    pub(super) global_flags: Vec<FlagSpec>,
    /// Bare flag words that are not `--long` options (`-V`, `--version`, ...).
    pub(super) global_words: Vec<String>,
    pub(super) commands: Vec<CommandSpec>,
    pub(super) slash: Vec<SlashEntry>,
}

mod model;
mod render;

use model::collect_model;

pub(crate) fn completions_command(args: &Args) -> Result<String, String> {
    let shell = args
        .positionals
        .first()
        .map(String::as_str)
        .unwrap_or_default();
    let model = collect_model();
    match shell {
        "bash" => Ok(model.render_bash()),
        "zsh" => Ok(model.render_zsh()),
        "fish" => Ok(model.render_fish()),
        other => Err(format!(
            "unknown shell '{}': usage: jeden completions <bash|zsh|fish>",
            if other.is_empty() { "<missing>" } else { other }
        )),
    }
}
