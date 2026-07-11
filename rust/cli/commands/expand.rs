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
        if let Some(end) = rest.find("\n---") {
            let after = &rest[end + "\n---".len()..];
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

fn replace_arg_slices(template: &str, positionals: &[String]) -> (String, bool) {
    let mut out = String::new();
    let mut rest = template;
    let mut used = false;
    let one = usize::from(true);
    while let Some(start) = rest.find("$@[") {
        out.push_str(&rest[..start]);
        let after = &rest[start + "$@[".len()..];
        let Some(end) = after.find(']') else {
            out.push_str(&rest[start..]);
            return (out, used);
        };
        let spec = &after[..end];
        let replacement = if let Some((raw_start, raw_len)) = spec.split_once(':') {
            let start = raw_start.trim().parse::<usize>().ok();
            let len = raw_len.trim().parse::<usize>().ok();
            start.zip(len).map(|(start, len)| {
                let from = start.saturating_sub(one).min(positionals.len());
                let to = from.saturating_add(len).min(positionals.len());
                positionals[from..to].join(" ")
            })
        } else {
            spec.trim().parse::<usize>().ok().map(|start| {
                let from = start.saturating_sub(one).min(positionals.len());
                positionals[from..].join(" ")
            })
        };
        if let Some(value) = replacement {
            out.push_str(&value);
            used = true;
        } else {
            out.push_str("$@[");
            out.push_str(spec);
            out.push(']');
        }
        rest = &after[end + "]".len()..];
    }
    out.push_str(rest);
    (out, used)
}

fn render_prompt_variables(
    template: &str,
    raw_args: &str,
    positionals: &[String],
) -> (String, bool) {
    let mut out = String::new();
    let mut rest = template;
    let mut used = false;
    while let Some(start) = rest.find("{{") {
        out.push_str(&rest[..start]);
        let after = &rest[start + "{{".len()..];
        let Some(end) = after.find("}}") else {
            out.push_str(&rest[start..]);
            return (out, used);
        };
        let expr = after[..end].trim();
        let value = match expr {
            "ARGUMENTS" | "arguments" => Some(raw_args.to_string()),
            "args" => Some(positionals.join(" ")),
            _ => {
                if let Some(index) = expr
                    .strip_prefix("args.")
                    .and_then(|raw| raw.parse::<usize>().ok())
                {
                    Some(positionals.get(index).cloned().unwrap_or_default())
                } else if let Some(raw) = expr
                    .strip_prefix("args[")
                    .and_then(|raw| raw.strip_suffix(']'))
                {
                    raw.parse::<usize>()
                        .ok()
                        .map(|index| positionals.get(index).cloned().unwrap_or_default())
                } else {
                    None
                }
            }
        };
        if let Some(value) = value {
            out.push_str(&value);
            used = true;
        } else {
            out.push_str("{{");
            out.push_str(&after[..end]);
            out.push_str("}}");
        }
        rest = &after[end + "}}".len()..];
    }
    out.push_str(rest);
    (out, used)
}

/// Expand a command template with args: `$ARGUMENTS`/`$@` = all args, `$1..$9` =
/// positionals. If the template uses no placeholder and args exist, they are
/// appended so a bare-body command still receives its arguments.
pub(crate) fn expand_file_command(template: &str, args: &str) -> String {
    let args = args.trim();
    let positionals = parse_command_args(args);
    let (mut out, used_slice) = replace_arg_slices(template, &positionals);
    let used_dollar_placeholder = used_slice
        || out.contains("$ARGUMENTS")
        || out.contains("$@")
        || DOLLAR_POSITIONALS.iter().any(|token| out.contains(token));
    out = out.replace("$ARGUMENTS", args).replace("$@", args);
    for (index, token) in DOLLAR_POSITIONALS.iter().enumerate() {
        out = out.replace(
            token,
            positionals.get(index).map(String::as_str).unwrap_or(""),
        );
    }
    let (rendered, used_prompt_variable) = render_prompt_variables(&out, args, &positionals);
    out = rendered;
    if !(used_dollar_placeholder || used_prompt_variable) && !args.is_empty() {
        out = format!("{}\n\n{}", out.trim_end(), args);
    }
    out
}

/// A file command resolved to its runnable prompt, or None. Public so both the
/// CLI run path and the interactive handler share one discovery.
pub(crate) fn resolve_file_command(cwd: &Path, command: &str, args: &str) -> Option<String> {
    find_file_command(cwd, command).map(|template| expand_file_command(&template, args))
}
