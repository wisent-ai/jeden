use unicode_segmentation::UnicodeSegmentation;
use unicode_width::UnicodeWidthStr;

use super::theme::{Emphasis, SemanticColor, Theme};

pub(super) fn paint(value: &str, style: &str, enabled: bool) -> String {
    let (token, emphasis) = match style {
        "dim" => (SemanticColor::TextMuted, Emphasis { dim: true, ..Emphasis::default() }),
        "bold" => (SemanticColor::TextPrimary, Emphasis { bold: true, ..Emphasis::default() }),
        "cyan" => (SemanticColor::Info, Emphasis::default()),
        "green" => (SemanticColor::Success, Emphasis::default()),
        "yellow" => (SemanticColor::Warning, Emphasis::default()),
        "magenta" => (SemanticColor::Accent, Emphasis::default()),
        "red" => (SemanticColor::Danger, Emphasis { bold: true, ..Emphasis::default() }),
        _ => (SemanticColor::TextPrimary, Emphasis::default()),
    };
    Theme::from_env(enabled).paint(value, token, emphasis)
}

pub(super) fn sanitize_terminal_text(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    let mut index = 0;
    while index < value.len() {
        let rest = &value[index..];
        if let Some(consumed) = terminal_sequence_len(rest) {
            index += consumed;
            continue;
        }
        let ch = rest.chars().next().expect("index remains on UTF-8 boundary");
        index += ch.len_utf8();
        match ch {
            '\n' => out.push('\n'),
            '\t' => out.push_str("  "),
            ch if !ch.is_control() && !matches!(ch as u32, 0x80..=0x9f) => out.push(ch),
            _ => {}
        }
    }
    out
}

pub(super) fn strip_terminal_controls(value: &str) -> String {
    sanitize_terminal_text(value)
}

pub(super) fn visible_len(value: &str) -> usize {
    UnicodeWidthStr::width(strip_terminal_controls(value).as_str())
}

pub(super) fn take_visible(value: &str, max: usize) -> String {
    if max == 0 { return String::new(); }
    let mut out = String::with_capacity(value.len().min(max.saturating_mul(4)));
    let mut width = 0;
    let mut index = 0;
    while index < value.len() {
        let rest = &value[index..];
        if let Some(consumed) = terminal_sequence_len(rest) {
            out.push_str(&rest[..consumed]);
            index += consumed;
            continue;
        }
        let grapheme = rest.graphemes(true).next().expect("non-empty remainder");
        let grapheme_width = UnicodeWidthStr::width(grapheme);
        if width + grapheme_width > max { break; }
        if grapheme.chars().all(|ch| !ch.is_control()) {
            out.push_str(grapheme);
            width += grapheme_width;
        }
        index += grapheme.len();
    }
    out
}

pub(super) fn pad_visible(value: &str, width: usize) -> String {
    let extra = width.saturating_sub(visible_len(value));
    format!("{}{}", value, " ".repeat(extra))
}

pub(super) fn wrap_line(line: &str, width: usize) -> Vec<String> {
    if width == 0 { return vec![String::new()]; }
    if visible_len(line) <= width { return vec![line.to_string()]; }
    let mut lines = Vec::new();
    let mut current = String::new();
    let mut current_width = 0;
    let mut index = 0;
    while index < line.len() {
        let rest = &line[index..];
        if let Some(consumed) = terminal_sequence_len(rest) {
            current.push_str(&rest[..consumed]);
            index += consumed;
            continue;
        }
        let grapheme = rest.graphemes(true).next().expect("non-empty remainder");
        index += grapheme.len();
        if grapheme.chars().any(char::is_control) { continue; }
        let grapheme_width = UnicodeWidthStr::width(grapheme);
        if current_width > 0 && current_width + grapheme_width > width {
            lines.push(std::mem::take(&mut current));
            current_width = 0;
        }
        current.push_str(grapheme);
        current_width += grapheme_width;
    }
    lines.push(current);
    lines
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
        if width == 0 { String::new() } else { format!("{}…", take_visible(value, width.saturating_sub(1))) }
    } else {
        value.to_string()
    }
}

fn terminal_sequence_len(value: &str) -> Option<usize> {
    let bytes = value.as_bytes();
    let first = *bytes.first()?;
    if first == 0x1b {
        let second = *bytes.get(1).unwrap_or(&0);
        return Some(match second {
            b'[' => csi_len(bytes),
            b']' => string_sequence_len(bytes),
            b'P' | b'X' | b'^' | b'_' => string_sequence_len(bytes),
            _ => bytes.len().min(2),
        });
    }
    if matches!(first, 0x90 | 0x98 | 0x9d | 0x9e | 0x9f) {
        return Some(string_sequence_len(bytes));
    }
    if first == 0x9b { return Some(csi_len(bytes)); }
    None
}

fn csi_len(bytes: &[u8]) -> usize {
    bytes.iter().position(|byte| (0x40..=0x7e).contains(byte) && *byte != 0x5b).map_or(bytes.len(), |index| index + 1)
}

fn string_sequence_len(bytes: &[u8]) -> usize {
    let mut index = 1;
    while index < bytes.len() {
        if bytes[index] == 0x07 || bytes[index] == 0x9c { return index + 1; }
        if bytes[index] == 0x1b && bytes.get(index + 1) == Some(&b'\\') { return index + 2; }
        index += 1;
    }
    bytes.len()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn native_tui_text_sanitizes_terminal_control_families_without_dropping_unicode() {
        for (name, input, expected) in [
            ("C0 and C1", "A\0B\u{0085}C\tD\nE", "ABC  D\nE"),
            ("CSI", "safe\x1b[31mred\x1b[0m!", "safered!"),
            ("OSC terminated by BEL", "a\x1b]0;forged title\x07b", "ab"),
            ("OSC terminated by ST", "a\x1b]52;c;secret\x1b\\b", "ab"),
            ("DCS", "a\x1bPmalicious payload\x1b\\b", "ab"),
            ("unicode", "Zażółć 👩🏽‍💻 世界", "Zażółć 👩🏽‍💻 世界"),
        ] {
            assert_eq!(sanitize_terminal_text(input), expected, "case: {name}");
        }
    }

    #[test]
    fn native_tui_text_visible_operations_never_split_extended_graphemes() {
        for (name, input, width, expected_take, expected_wrap) in [
            ("combining mark", "e\u{301}x", 1, "e\u{301}", vec!["e\u{301}", "x"]),
            ("emoji modifier", "👍🏽x", 2, "👍🏽", vec!["👍🏽", "x"]),
            ("ZWJ emoji", "👩🏽‍💻x", 2, "👩🏽‍💻", vec!["👩🏽‍💻", "x"]),
            ("wide CJK", "界x", 2, "界", vec!["界", "x"]),
        ] {
            assert_eq!(take_visible(input, width), expected_take, "take case: {name}");
            assert_eq!(wrap_line(input, width), expected_wrap, "wrap case: {name}");
        }
    }
}
