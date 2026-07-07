pub(super) fn paint(value: &str, color: &str, enabled: bool) -> String {
    if !enabled {
        return value.to_string();
    }
    let code = match color {
        "dim" => "\x1b[2m",
        "bold" => "\x1b[1m",
        "cyan" => "\x1b[36m",
        "green" => "\x1b[32m",
        "yellow" => "\x1b[33m",
        "magenta" => "\x1b[35m",
        "red" => "\x1b[31m",
        _ => "",
    };
    format!("{}{}\x1b[0m", code, value)
}

pub(super) fn visible_len(value: &str) -> usize {
    let mut len = 0;
    let mut chars = value.chars().peekable();
    while let Some(ch) = chars.next() {
        if ch == '\x1b' && chars.peek() == Some(&'[') {
            chars.next();
            for code in chars.by_ref() {
                if code == 'm' {
                    break;
                }
            }
        } else {
            len += 1;
        }
    }
    len
}

pub(super) fn take_visible(value: &str, max: usize) -> String {
    value.chars().take(max).collect()
}

pub(super) fn pad_visible(value: &str, width: usize) -> String {
    let extra = width.saturating_sub(visible_len(value));
    format!("{}{}", value, " ".repeat(extra))
}

pub(super) fn wrap_line(line: &str, width: usize) -> Vec<String> {
    if visible_len(line) <= width {
        return vec![line.to_string()];
    }
    let chars: Vec<char> = line.chars().collect();
    if width == 0 {
        return vec![String::new()];
    }
    chars.chunks(width).map(|chunk| chunk.iter().collect()).collect()
}

pub(super) fn compact_path(cwd: &str) -> String {
    let parts = cwd.split('/').filter(|part| !part.is_empty()).collect::<Vec<_>>();
    if parts.len() >= 2 {
        format!("…/{}/{}", parts[parts.len() - 2], parts[parts.len() - 1])
    } else {
        cwd.to_string()
    }
}

pub(super) fn clamp_visible(value: &str, width: usize) -> String {
    if visible_len(value) > width {
        format!("{}…", take_visible(value, width.saturating_sub(1)))
    } else {
        value.to_string()
    }
}
