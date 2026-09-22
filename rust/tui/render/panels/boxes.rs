//! Drawing a bordered panel in a terminal, single pane and split.
//!
//! Split out of `tui/render/mod.rs`, which had grown past the module line cap.

use crate::tui::text::{clamp_visible, sanitize_terminal_text, visible_len, wrap_line};
use crate::tui::text::paint;

pub(crate) fn pad_visible(value: &str, width: usize) -> String {
    let mut padded = clamp_visible(value, width);
    padded.extend(std::iter::repeat_n(
        ' ',
        width.saturating_sub(visible_len(&padded)),
    ));
    padded
}

pub(crate) fn framed_header(label: &str, width: usize, color: bool) -> String {
    let middle_width = width.saturating_sub(2);
    let label = clamp_visible(
        &format!(" {} ", sanitize_terminal_text(label)),
        middle_width,
    );
    let rule_width = middle_width.saturating_sub(visible_len(&label));
    format!(
        "{}{}{}{}",
        paint("╭", "cyan", color),
        paint(&label, "bold", color),
        paint(&"─".repeat(rule_width), "cyan", color),
        paint("╮", "cyan", color),
    )
}

pub(crate) fn input_prefix_width(width: usize) -> usize {
    width.saturating_sub(1).min(2)
}

pub(crate) fn boxed(title: &str, rows: &[String], width: usize, color: bool) -> Vec<String> {
    let width = width.max(1);
    let title = sanitize_terminal_text(title);
    if width < 6 {
        let mut out = vec![paint(&clamp_visible(&title, width), "bold", color)];
        for row in rows {
            let safe = sanitize_terminal_text(row);
            for logical in safe.split('\n') {
                out.extend(wrap_line(logical, width));
            }
        }
        return out;
    }

    let inner_width = width - 4;
    let mut out = vec![framed_header(&title, width, color)];
    for row in rows {
        let safe = sanitize_terminal_text(row);
        for logical in safe.split('\n') {
            for part in wrap_line(logical, inner_width) {
                out.push(format!(
                    "{} {} {}",
                    paint("│", "cyan", color),
                    pad_visible(&part, inner_width),
                    paint("│", "cyan", color),
                ));
            }
        }
    }
    out.push(format!(
        "{}{}{}",
        paint("╰", "cyan", color),
        paint(&"─".repeat(width - 2), "cyan", color),
        paint("╯", "cyan", color),
    ));
    out
}

/// Two-pane frame: the divider is part of the border, joined with `┬` at the
/// top and `┴` above the footer, the way omp draws its model hub. Drawing the
/// split as a character inside the row text (the first attempt here) looks
/// almost right and lines up with nothing.
pub(crate) fn boxed_split(
    title: &str,
    pane: usize,
    rows: &[(String, String)],
    footer: &str,
    width: usize,
    color: bool,
) -> Vec<String> {
    let width = width.max(usize::from(true));
    let frame = "│┬┴├┤╰╯─".len();
    if width < frame || pane + frame >= width {
        let flat = rows
            .iter()
            .map(|(left, right)| format!("{left} {right}"))
            .chain(std::iter::once(footer.to_string()))
            .collect::<Vec<_>>();
        return boxed(title, &flat, width, color);
    }
    let inner = width - "││".len();
    let right_width = inner - pane - "┬".len();
    // The title sits over the pane that can hold it: a long one would be
    // clipped to "Select model ro…" in the narrow brands column.
    let framed = format!(" {} ", sanitize_terminal_text(title));
    let title_left = visible_len(&framed) <= pane;
    let label = clamp_visible(&framed, if title_left { pane } else { right_width });
    let head_pad = if title_left { pane } else { right_width }.saturating_sub(visible_len(&label));
    let (left_head, right_head) = if title_left {
        (
            format!("{}{}", paint(&label, "bold", color), "─".repeat(head_pad)),
            "─".repeat(right_width),
        )
    } else {
        (
            "─".repeat(pane),
            format!("{}{}", paint(&label, "bold", color), "─".repeat(head_pad)),
        )
    };
    let mut out = vec![format!(
        "{}{}{}{}{}",
        paint("╭", "cyan", color),
        paint(&left_head, "cyan", color),
        paint("┬", "cyan", color),
        paint(&right_head, "cyan", color),
        paint("╮", "cyan", color),
    )];
    // One space of padding inside every cell, so text never touches the rules.
    let gutter = " ";
    for (left, right) in rows {
        out.push(format!(
            "{}{}{}{}{}{}{}",
            paint("│", "cyan", color),
            gutter,
            pad_visible(left, pane.saturating_sub(gutter.len())),
            paint("│", "cyan", color),
            gutter,
            pad_visible(right, right_width.saturating_sub(gutter.len())),
            paint("│", "cyan", color),
        ));
    }
    out.push(format!(
        "{}{}{}{}{}",
        paint("├", "cyan", color),
        paint(&"─".repeat(pane), "cyan", color),
        paint("┴", "cyan", color),
        paint(&"─".repeat(right_width), "cyan", color),
        paint("┤", "cyan", color),
    ));
    out.push(format!(
        "{}{}{}{}",
        paint("│", "cyan", color),
        gutter,
        pad_visible(footer, inner.saturating_sub(gutter.len())),
        paint("│", "cyan", color),
    ));
    out.push(format!(
        "{}{}{}",
        paint("╰", "cyan", color),
        paint(&"─".repeat(inner), "cyan", color),
        paint("╯", "cyan", color),
    ));
    out
}
