//! Shell completion script generation.
//!
//! Everything is derived from two in-repo metadata tables so future commands
//! come free: the `jeden --help` usage text builder (`crate::usage`, the CLI
//! usage surface) and the builtin slash-command registry
//! (`capability::builtin_slash_specs`, including aliases). No shell script is
//! hand-written per command.

use crate::capability::builtin_slash_specs;
use crate::{usage, Args};

#[derive(Clone, Debug)]
struct FlagSpec {
    /// Long form, e.g. `--cwd`. Short flags keep their single dash.
    name: String,
    takes_value: bool,
}

#[derive(Clone, Debug, Default)]
struct CommandSpec {
    name: String,
    description: String,
    flags: Vec<FlagSpec>,
    /// Sub-action words parsed from the first `<a|b|c>` / `[a|b|c]` group.
    actions: Vec<String>,
}

#[derive(Clone, Debug)]
struct SlashEntry {
    /// `/name` or `/alias`.
    word: String,
    description: String,
}

#[derive(Clone, Debug, Default)]
struct CompletionModel {
    global_flags: Vec<FlagSpec>,
    /// Bare flag words that are not `--long` options (`-V`, `--version`, ...).
    global_words: Vec<String>,
    commands: Vec<CommandSpec>,
    slash: Vec<SlashEntry>,
}

fn push_flag(flags: &mut Vec<FlagSpec>, name: &str, takes_value: bool) {
    if name.is_empty() {
        return;
    }
    if let Some(existing) = flags.iter_mut().find(|flag| flag.name == name) {
        existing.takes_value |= takes_value;
    } else {
        flags.push(FlagSpec {
            name: name.to_string(),
            takes_value,
        });
    }
}

fn push_word(words: &mut Vec<String>, word: &str) {
    if !word.is_empty() && !words.iter().any(|v| v == word) {
        words.push(word.to_string());
    }
}

/// A token is a value placeholder when it is not itself a flag, bracket, or
/// alternation separator (`path`, `n`, `name`, `"task"`, `<key>`, ...).
fn is_value_token(token: &str) -> bool {
    let token = token.trim_matches(|c| matches!(c, '[' | ']'));
    !token.is_empty() && !token.starts_with('-') && token != "|"
}

/// Collect `--long` flags from one usage line. Handles compact alternations
/// such as `[--yolo|--auto-approve]` and value placeholders after a flag.
fn collect_flags(line: &str, flags: &mut Vec<FlagSpec>) {
    let tokens: Vec<&str> = line.split_whitespace().collect();
    for (index, token) in tokens.iter().enumerate() {
        let cleaned = token.trim_matches(|c| matches!(c, '[' | ']'));
        if !cleaned.starts_with('-') {
            continue;
        }
        let takes_value = !cleaned.contains('|')
            && tokens
                .get(index + 1)
                .map(|next| is_value_token(next))
                .unwrap_or(false);
        for part in cleaned.split('|') {
            if part.starts_with("--") {
                push_flag(flags, part, takes_value);
            } else if part.starts_with('-') {
                push_flag(flags, part, false);
            }
        }
    }
}

/// Extract sub-action words from the first `<a|b|c>` or `[a|b|c]` group that
/// contains an alternation, e.g. `config [list|path|get <key>|...]` yields
/// `list path get set reset`.
fn extract_actions(line: &str) -> Vec<String> {
    let mut actions = Vec::new();
    let mut start = None;
    let mut opener = '[';
    for (index, ch) in line.char_indices() {
        if matches!(ch, '[' | '<') {
            start = Some(index + 1);
            opener = ch;
            break;
        }
    }
    let Some(start) = start else { return actions };
    let closer = if opener == '[' { ']' } else { '>' };
    let Some(end) = line[start..].find(closer).map(|i| start + i) else {
        return actions;
    };
    let group = &line[start..end];
    if !group.contains('|') {
        return actions;
    }
    for alternative in group.split('|') {
        let word = alternative
            .split_whitespace()
            .next()
            .unwrap_or("")
            .trim_matches(|c| matches!(c, '[' | ']' | '<' | '>' | '"'));
        if !word.is_empty() && !word.starts_with('-') {
            push_word(&mut actions, word);
        }
    }
    actions
}

/// Parse the `jeden --help` usage text into a completion model. Each line of
/// the form `jeden <cmd> ...` contributes one command; the bracketed global
/// flag line and the `--version | -V` line contribute global words.
fn parse_usage(text: &str) -> (Vec<FlagSpec>, Vec<String>, Vec<CommandSpec>) {
    let mut global_flags = Vec::new();
    let mut global_words = Vec::new();
    let mut commands = Vec::new();
    for line in text.lines() {
        let trimmed = line.trim();
        let Some(rest) = trimmed.strip_prefix("jeden ") else {
            continue;
        };
        let Some(name) = rest.split_whitespace().next() else {
            continue;
        };
        if name.starts_with('[') {
            // Global option line: jeden [--cwd path] [--model name] ...
            collect_flags(rest, &mut global_flags);
            continue;
        }
        if name.starts_with('-') {
            // jeden --version | -V
            for token in rest.split_whitespace() {
                if token.starts_with('-') {
                    push_word(&mut global_words, token);
                }
            }
            continue;
        }
        let mut flags = Vec::new();
        collect_flags(rest, &mut flags);
        let after = rest[name.len()..].trim_start();
        let description = if after.is_empty()
            || after.starts_with('[')
            || after.starts_with('<')
            || after.starts_with('"')
            || after.starts_with("--")
        {
            String::new()
        } else {
            after.to_string()
        };
        commands.push(CommandSpec {
            name: name.to_string(),
            description,
            flags,
            actions: extract_actions(rest),
        });
    }
    (global_flags, global_words, commands)
}

fn collect_model() -> CompletionModel {
    let (global_flags, mut global_words, commands) = parse_usage(&usage());
    // Handled by parse_args but intentionally not shown in the usage text.
    push_word(&mut global_words, "--help");
    push_word(&mut global_words, "-h");
    let mut slash = Vec::new();
    for spec in builtin_slash_specs() {
        slash.push(SlashEntry {
            word: format!("/{}", spec.name),
            description: spec.description.to_string(),
        });
        for alias in spec.aliases {
            slash.push(SlashEntry {
                word: format!("/{alias}"),
                description: format!("{} (alias for /{})", spec.description, spec.name),
            });
        }
    }
    CompletionModel {
        global_flags,
        global_words,
        commands,
        slash,
    }
}

impl CompletionModel {
    fn command(&self, name: &str) -> Option<&CommandSpec> {
        self.commands.iter().find(|command| command.name == name)
    }

    fn subcommand_words(&self) -> String {
        self.commands
            .iter()
            .map(|command| command.name.as_str())
            .collect::<Vec<_>>()
            .join(" ")
    }

    fn global_words(&self) -> String {
        let mut words: Vec<&str> = self
            .global_flags
            .iter()
            .map(|flag| flag.name.as_str())
            .collect();
        words.extend(self.global_words.iter().map(String::as_str));
        words.join(" ")
    }

    fn flag_words(command: &CommandSpec) -> String {
        command
            .flags
            .iter()
            .map(|flag| flag.name.as_str())
            .collect::<Vec<_>>()
            .join(" ")
    }

    fn slash_words(&self) -> String {
        self.slash
            .iter()
            .map(|entry| entry.word.as_str())
            .collect::<Vec<_>>()
            .join(" ")
    }

    /// bash: top-level word list plus per-subcommand word lists via a case
    /// dispatch (the standard way to keep `complete -W`-style lists per
    /// subcommand; a bare `complete -W` cannot dispatch on position).
    fn render_bash(&self) -> String {
        let mut out = String::new();
        out.push_str(
            "# bash completion for jeden — generated by `jeden completions bash`; do not edit.\n",
        );
        out.push_str("_jeden_completions() {\n");
        out.push_str("    local cur\n");
        out.push_str("    cur=\"${COMP_WORDS[COMP_CWORD]}\"\n");
        out.push_str(&format!(
            "    local subcommands=\"{}\"\n",
            self.subcommand_words()
        ));
        out.push_str(&format!(
            "    local global_flags=\"{}\"\n",
            self.global_words()
        ));
        out.push_str("    if [[ ${COMP_CWORD} -eq 1 ]]; then\n");
        out.push_str("        COMPREPLY=( $(compgen -W \"${subcommands} ${global_flags}\" -- \"${cur}\") )\n");
        out.push_str("        return 0\n");
        out.push_str("    fi\n");
        out.push_str("    case \"${COMP_WORDS[1]}\" in\n");
        for command in &self.commands {
            let mut words: Vec<String> = Vec::new();
            if command.name == "run" {
                words.push(self.slash_words());
            }
            if !command.actions.is_empty() {
                words.push(command.actions.join(" "));
            }
            let flags = Self::flag_words(command);
            if !flags.is_empty() {
                words.push(flags);
            }
            if words.is_empty() {
                continue;
            }
            out.push_str(&format!("        {})\n", command.name));
            out.push_str(&format!(
                "            COMPREPLY=( $(compgen -W \"{}\" -- \"${{cur}}\") ) ;;\n",
                words.join(" ")
            ));
        }
        out.push_str("        *)\n");
        out.push_str(
            "            COMPREPLY=( $(compgen -W \"${global_flags}\" -- \"${cur}\") ) ;;\n",
        );
        out.push_str("    esac\n");
        out.push_str("}\n");
        out.push_str("complete -F _jeden_completions jeden\n");
        out
    }

    /// zsh: `#compdef jeden` with `_arguments` for top-level subcommands and
    /// flags, and slash commands offered for `jeden run`.
    fn render_zsh(&self) -> String {
        let mut out = String::new();
        out.push_str("#compdef jeden\n");
        out.push_str(
            "# zsh completion for jeden — generated by `jeden completions zsh`; do not edit.\n",
        );
        out.push_str("_jeden() {\n");
        out.push_str("    local state\n");
        out.push_str("    local -a line commands\n");
        out.push_str("    typeset -A opt_args\n");
        out.push_str("    commands=(\n");
        for command in &self.commands {
            if command.description.is_empty() {
                out.push_str(&format!("        '{}'\n", zsh_escape(&command.name)));
            } else {
                out.push_str(&format!(
                    "        '{}:{}'\n",
                    zsh_escape(&command.name),
                    zsh_escape(&command.description)
                ));
            }
        }
        out.push_str("    )\n");
        out.push_str("    _arguments -C \\\n");
        for flag in &self.global_flags {
            let name = zsh_escape(&flag.name);
            if flag.takes_value {
                if flag.name == "--cwd" {
                    out.push_str(&format!(
                        "        '{name}[working directory]:directory:_files -/' \\\n"
                    ));
                } else {
                    out.push_str(&format!("        '{name}[expects a value]:value:' \\\n"));
                }
            } else {
                out.push_str(&format!("        '{name}' \\\n"));
            }
        }
        for word in &self.global_words {
            let desc = match word.as_str() {
                "--version" | "-V" => "[print version]",
                "--help" | "-h" => "[show usage]",
                _ => "",
            };
            out.push_str(&format!("        '{}{desc}' \\\n", zsh_escape(word)));
        }
        out.push_str("        '1: :->command' \\\n");
        out.push_str("        '*:: :->args' && return 0\n");
        out.push_str("    case \"${state}\" in\n");
        out.push_str("        command)\n");
        out.push_str("            _describe -t jeden-commands 'jeden command' commands\n");
        out.push_str("            ;;\n");
        out.push_str("        args)\n");
        out.push_str("            case \"${line[1]}\" in\n");
        if self.command("run").is_some() {
            out.push_str("                run)\n");
            out.push_str("                    local -a slash_commands\n");
            out.push_str("                    slash_commands=(\n");
            for entry in &self.slash {
                out.push_str(&format!(
                    "                        '{}:{}'\n",
                    zsh_escape(&entry.word),
                    zsh_escape(&entry.description)
                ));
            }
            out.push_str("                    )\n");
            out.push_str(
                "                    _describe -t jeden-slash-commands 'slash command' slash_commands\n",
            );
            out.push_str("                    ;;\n");
        }
        for command in &self.commands {
            if command.actions.is_empty() {
                continue;
            }
            let actions = command
                .actions
                .iter()
                .map(|action| format!("'{}'", zsh_escape(action)))
                .collect::<Vec<_>>()
                .join(" ");
            out.push_str(&format!("                {})\n", command.name));
            out.push_str(&format!(
                "                    _values '{} action' {}\n",
                command.name, actions
            ));
            out.push_str("                    ;;\n");
        }
        out.push_str("            esac\n");
        out.push_str("            ;;\n");
        out.push_str("    esac\n");
        out.push_str("}\n");
        out.push_str("_jeden \"$@\"\n");
        out
    }

    /// fish: one `complete -c jeden` line per subcommand/flag, plus lines
    /// scoped with `__fish_seen_subcommand_from` for actions and slash words.
    fn render_fish(&self) -> String {
        let mut out = String::new();
        out.push_str(
            "# fish completion for jeden — generated by `jeden completions fish`; do not edit.\n",
        );
        for command in &self.commands {
            out.push_str("complete -c jeden -n '__fish_use_subcommand'");
            out.push_str(&format!(" -a '{}'", fish_escape(&command.name)));
            if !command.description.is_empty() {
                out.push_str(&format!(" -d '{}'", fish_escape(&command.description)));
            }
            out.push('\n');
        }
        for flag in &self.global_flags {
            out.push_str("complete -c jeden -n '__fish_use_subcommand'");
            if let Some(short) = flag.name.strip_prefix('-').filter(|s| !s.starts_with('-')) {
                out.push_str(&format!(" -s '{}'", fish_escape(short)));
            } else if let Some(long) = flag.name.strip_prefix("--") {
                out.push_str(&format!(" -l '{}'", fish_escape(long)));
            }
            if flag.takes_value {
                out.push_str(" -r");
            }
            out.push('\n');
        }
        for word in &self.global_words {
            out.push_str("complete -c jeden -n '__fish_use_subcommand'");
            if let Some(long) = word.strip_prefix("--") {
                out.push_str(&format!(" -l '{}'", fish_escape(long)));
            } else if let Some(short) = word.strip_prefix('-') {
                out.push_str(&format!(" -s '{}'", fish_escape(short)));
            }
            out.push('\n');
        }
        for entry in &self.slash {
            out.push_str(&format!(
                "complete -c jeden -n '__fish_seen_subcommand_from run' -a '{}' -d '{}'\n",
                fish_escape(&entry.word),
                fish_escape(&entry.description)
            ));
        }
        for command in &self.commands {
            if !command.actions.is_empty() {
                out.push_str(&format!(
                    "complete -c jeden -n '__fish_seen_subcommand_from {}' -a '{}'\n",
                    command.name,
                    command
                        .actions
                        .iter()
                        .map(|action| fish_escape(action))
                        .collect::<Vec<_>>()
                        .join(" ")
                ));
            }
            for flag in &command.flags {
                let Some(long) = flag.name.strip_prefix("--") else {
                    continue;
                };
                // Global flags are already emitted unscoped above.
                if self.global_flags.iter().any(|g| g.name == flag.name) {
                    continue;
                }
                out.push_str(&format!(
                    "complete -c jeden -n '__fish_seen_subcommand_from {}' -l '{}'",
                    command.name,
                    fish_escape(long)
                ));
                if flag.takes_value {
                    out.push_str(" -r");
                }
                out.push('\n');
            }
        }
        out
    }
}

/// Escape a string for zsh single-quoted words (`'` → `'\''`).
fn zsh_escape(value: &str) -> String {
    value.replace('\'', "'\\''")
}

/// Escape a string for fish single-quoted words (`\` → `\\`, `'` → `\'`).
fn fish_escape(value: &str) -> String {
    value.replace('\\', "\\\\").replace('\'', "\\'")
}

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
