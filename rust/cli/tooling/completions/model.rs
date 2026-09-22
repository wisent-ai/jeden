//! Reading the command line surface out of the product's own help text and
//! slash registry.
//!
//! Split out of `cli/tooling/completions.rs`, which had grown past the module
//! line cap.

use super::{CommandSpec, CompletionModel, FlagSpec, SlashEntry};
use crate::capability::builtin_slash_specs;
use crate::usage;

pub(super) fn push_flag(flags: &mut Vec<FlagSpec>, name: &str, takes_value: bool) {
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

pub(super) fn push_word(words: &mut Vec<String>, word: &str) {
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

pub(super) fn collect_model() -> CompletionModel {
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
        for alias in &spec.aliases {
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
