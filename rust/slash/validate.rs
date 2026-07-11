/// Validate a Jeden plugin/marketplace name: lowercase alphanumeric plus `-`/`.`,
/// first and last char alphanumeric.
pub(super) fn valid_component_name(name: &str) -> bool {
    match (name.chars().next(), name.chars().next_back()) {
        (Some(first), Some(last))
            if first.is_ascii_alphanumeric() && last.is_ascii_alphanumeric() =>
        {
            name.chars()
                .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-' || c == '.')
        }
        _ => false,
    }
}
pub(super) fn valid_plugin_name(name: &str) -> bool {
    valid_component_name(name)
}
pub(super) fn valid_marketplace_name(name: &str) -> bool {
    valid_component_name(name)
}
/// A plugin id is `plugin@marketplace`, each part a valid name.
pub(super) fn valid_plugin_id(id: &str) -> bool {
    match id.split_once('@') {
        Some((plugin, mkt)) => valid_plugin_name(plugin) && valid_marketplace_name(mkt),
        None => false,
    }
}

/// Reject anything unsafe to hand to `git` as an argument: empty, an option-like
/// leading `-`, control chars, or shell metacharacters. `git` is invoked via
/// `Command` (no shell) but this is defense in depth against injection through
/// catalog-controlled URLs/refs/paths.
pub(super) fn git_arg_safe(value: &str) -> bool {
    if value.is_empty() || value.starts_with('-') {
        return false;
    }
    !value.chars().any(|c| {
        c.is_control()
            || matches!(
                c,
                ' ' | '\t'
                    | '\n'
                    | '\r'
                    | ';'
                    | '&'
                    | '|'
                    | '`'
                    | '$'
                    | '('
                    | ')'
                    | '{'
                    | '}'
                    | '<'
                    | '>'
                    | '*'
                    | '?'
                    | '['
                    | ']'
                    | '!'
                    | '\\'
                    | '\''
                    | '"'
            )
    })
}
