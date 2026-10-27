//! Command template expansion: frontmatter stripping, argument slices, and prompt variables.

use std::path::Path;

use super::find_file_command;

/// The `$1`..`$9` positional placeholder tokens. Kept as a length-unannotated
/// slice of string literals so the substitution loop needs no bare numeric
/// literal; the array index is the zero-based positional index.
const DOLLAR_POSITIONALS: &[&str] = &["$1", "$2", "$3", "$4", "$5", "$6", "$7", "$8", "$9"];

/// Drop a leading `---\n...\n---` YAML frontmatter block if present.
pub(crate) fn strip_frontmatter(text: &str) -> String {
    let trimmed = text.trim_start_matches("\u{feff}");
    if let Some(rest) = trimmed.strip_prefix("---\n") {
        if let Some((_, after)) = rest.split_once("\n---") {
            return after.trim_start_matches('\n').to_string();
        }
    }
    text.to_string()
}

fn parse_command_args(args: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut current = String::new();
    let mut quote: Option<char> = None;
    for ch in args.chars() {
        if let Some(q) = quote {
            if ch == q {
                quote = None;
            } else {
                current.push(ch);
            }
        } else if ch == '\'' || ch == '"' {
            quote = Some(ch);
        } else if ch.is_whitespace() {
            if !current.is_empty() {
                out.push(std::mem::take(&mut current));
            }
        } else {
            current.push(ch);
        }
    }
    if !current.is_empty() {
        out.push(current);
    }
    out
}

/// The whole number `raw` writes. Text that is not digits names no position,
/// so the placeholder holding it is none and stays as written; digits too
/// large to be a position are refused naming the placeholder.
fn whole_number(raw: &str, placeholder: &str) -> Result<Option<usize>, String> {
    let digits = raw.trim();
    if digits.is_empty() || !digits.chars().all(|ch| ch.is_ascii_digit()) {
        return Ok(None);
    }
    digits
        .parse::<usize>()
        .map(Some)
        .map_err(|error| format!("the command template's placeholder {placeholder} names no argument position: {error}"))
}

/// The positional argument at zero-based `index`; a placeholder for an
/// argument the invocation did not give expands to nothing, as in a shell.
fn positional(positionals: &[String], index: usize) -> String {
    match positionals.get(index) {
        Some(value) => value.clone(),
        None => String::new(),
    }
}

/// The arguments from position `start` on, counted from one as `$1` is (a
/// position before the first reads from the first), at most `length` of them
/// when a length is given.
fn argument_slice(positionals: &[String], start: usize, length: Option<usize>) -> String {
    let from_start = positionals
        .iter()
        .enumerate()
        .filter(|(index, _)| positionals[..=*index].len() >= start)
        .map(|(_, value)| value.as_str());
    let taken: Vec<&str> = match length {
        Some(length) => from_start.take(length).collect(),
        None => from_start.collect(),
    };
    taken.join(" ")
}

fn replace_arg_slices(template: &str, positionals: &[String]) -> Result<(String, bool), String> {
    let mut out = String::new();
    let mut rest = template;
    let mut used = false;
    while let Some((before, after)) = rest.split_once("$@[") {
        out.push_str(before);
        let Some((spec, remaining)) = after.split_once(']') else {
            out.push_str("$@[");
            out.push_str(after);
            return Ok((out, used));
        };
        let placeholder = format!("$@[{spec}]");
        // A spec that is not `start` or `start:length` in whole numbers is no
        // slice: it stays in the prompt as written.
        let replacement = match spec.split_once(':') {
            Some((raw_start, raw_length)) => match (
                whole_number(raw_start, &placeholder)?,
                whole_number(raw_length, &placeholder)?,
            ) {
                (Some(start), Some(length)) => Some(argument_slice(positionals, start, Some(length))),
                _ => None,
            },
            None => whole_number(spec, &placeholder)?.map(|start| argument_slice(positionals, start, None)),
        };
        match replacement {
            Some(value) => {
                out.push_str(&value);
                used = true;
            }
            None => out.push_str(&placeholder),
        }
        rest = remaining;
    }
    out.push_str(rest);
    Ok((out, used))
}

fn render_prompt_variables(
    template: &str,
    raw_args: &str,
    positionals: &[String],
) -> Result<(String, bool), String> {
    let mut out = String::new();
    let mut rest = template;
    let mut used = false;
    while let Some((before, after)) = rest.split_once("{{") {
        out.push_str(before);
        let Some((written, remaining)) = after.split_once("}}") else {
            out.push_str("{{");
            out.push_str(after);
            return Ok((out, used));
        };
        let placeholder = format!("{{{{{written}}}}}");
        let expr = written.trim();
        let value = match expr {
            // The command-file convention's own name (Claude Code commands:
            // `$ARGUMENTS`); the lowercase second spelling is gone.
            "ARGUMENTS" => Some(raw_args.to_string()),
            "args" => Some(positionals.join(" ")),
            _ => {
                let index = match (expr.strip_prefix("args."), expr.strip_prefix("args[").and_then(|raw| raw.strip_suffix(']'))) {
                    (Some(raw), _) | (None, Some(raw)) => whole_number(raw, &placeholder)?,
                    (None, None) => None,
                };
                index.map(|index| positional(positionals, index))
            }
        };
        match value {
            Some(value) => {
                out.push_str(&value);
                used = true;
            }
            None => out.push_str(&placeholder),
        }
        rest = remaining;
    }
    out.push_str(rest);
    Ok((out, used))
}

/// Expand a command template with args: `$ARGUMENTS`/`$@` = all args, `$1..$9` =
/// positionals. If the template uses no placeholder and args exist, they are
/// appended so a bare-body command still receives its arguments.
pub(crate) fn expand_file_command(template: &str, args: &str) -> Result<String, String> {
    let args = args.trim();
    let positionals = parse_command_args(args);
    let (mut out, used_slice) = replace_arg_slices(template, &positionals)?;
    let used_dollar_placeholder = used_slice
        || out.contains("$ARGUMENTS")
        || out.contains("$@")
        || DOLLAR_POSITIONALS.iter().any(|token| out.contains(token));
    out = out.replace("$ARGUMENTS", args).replace("$@", args);
    for (index, token) in DOLLAR_POSITIONALS.iter().enumerate() {
        out = out.replace(token, &positional(&positionals, index));
    }
    let (rendered, used_prompt_variable) = render_prompt_variables(&out, args, &positionals)?;
    out = rendered;
    if !(used_dollar_placeholder || used_prompt_variable) && !args.is_empty() {
        out = format!("{}\n\n{}", out.trim_end(), args);
    }
    Ok(out)
}

/// A file command resolved to its runnable prompt, or none when no provider
/// declares it; a declared command whose file cannot be read, or whose
/// template names no argument position, is refused naming it. The CLI run
/// path and the interactive handler share this one discovery.
pub(crate) fn resolve_file_command(
    cwd: &Path,
    command: &str,
    args: &str,
) -> Result<Option<String>, String> {
    match find_file_command(cwd, command)? {
        Some(template) => expand_file_command(&template, args).map(Some),
        None => Ok(None),
    }
}
