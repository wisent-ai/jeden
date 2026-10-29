//! Drawing a bordered panel in a terminal, single pane and split.
//!
//! Every width here is measured in terminal columns with `visible_len`: the
//! box glyphs are three bytes each, so a byte count made the split pane four
//! columns narrower than its frame.
//!
//! Split out of `tui/render/mod.rs`, which had grown past the module line cap.

use crate::tui::text::paint;
use crate::tui::text::{clamp_visible, sanitize_terminal_text, visible_len, wrap_line};

const BORDER: &str = "cyan";
const VERTICAL: &str = "│";
const HORIZONTAL: &str = "─";
const TOP_LEFT: &str = "╭";
const TOP_RIGHT: &str = "╮";
const BOTTOM_LEFT: &str = "╰";
const BOTTOM_RIGHT: &str = "╯";
const SPLIT_TOP: &str = "┬";
const SPLIT_BOTTOM: &str = "┴";
const JOIN_LEFT: &str = "├";
const JOIN_RIGHT: &str = "┤";
/// One space of padding inside every cell, so text never touches the rules.
const GUTTER: &str = " ";
/// What the prompt line draws before the input.
pub(crate) const INPUT_PREFIX: &str = "> ";

pub(crate) fn pad_visible(value: &str, width: usize) -> String {
    let mut padded = clamp_visible(value, width);
    padded.extend(std::iter::repeat_n(
        ' ',
        width.saturating_sub(visible_len(&padded)),
    ));
    padded
}

/// The columns a frame's two corners leave for what lies between them.
fn between_corners(width: usize, left: &str, right: &str) -> usize {
    width.saturating_sub(visible_len(left) + visible_len(right))
}

pub(crate) fn framed_header(label: &str, width: usize, color: bool) -> String {
    let middle_width = between_corners(width, TOP_LEFT, TOP_RIGHT);
    let label = clamp_visible(
        &format!(" {} ", sanitize_terminal_text(label)),
        middle_width,
    );
    let rule_width = middle_width.saturating_sub(visible_len(&label));
    format!(
        "{}{}{}{}",
        paint(TOP_LEFT, BORDER, color),
        paint(&label, "bold", color),
        paint(&HORIZONTAL.repeat(rule_width), BORDER, color),
        paint(TOP_RIGHT, BORDER, color),
    )
}

/// The columns the input prefix takes, leaving at least one for the input.
pub(crate) fn input_prefix_width(width: usize) -> usize {
    visible_len(INPUT_PREFIX).min(width.saturating_sub(usize::from(crate::tally::STEP)))
}

/// The columns a framed row leaves its text: a row is a border, a gutter,
/// the text, a gutter and a border. `None` when `width` leaves the text no
/// column, and so is drawn unframed.
pub(crate) fn frame_inner(width: usize) -> Option<usize> {
    let chrome =
        visible_len(VERTICAL) + visible_len(GUTTER) + visible_len(GUTTER) + visible_len(VERTICAL);
    width
        .checked_sub(chrome)
        .filter(|inner| *inner > crate::tally::ORIGIN)
}

/// What a framed prompt draws before its first input row: the frame's
/// bottom-left corner turning into its rule.
pub(crate) fn prompt_corner() -> String {
    format!("{BOTTOM_LEFT}{HORIZONTAL}")
}

pub(crate) fn boxed(title: &str, rows: &[String], width: usize, color: bool) -> Vec<String> {
    let title = sanitize_terminal_text(title);
    let Some(inner_width) = frame_inner(width) else {
        let width = width.max(usize::from(crate::tally::STEP));
        let mut out = vec![paint(&clamp_visible(&title, width), "bold", color)];
        for row in rows {
            let safe = sanitize_terminal_text(row);
            for logical in safe.split('\n') {
                out.extend(wrap_line(logical, width));
            }
        }
        return out;
    };
    let mut out = vec![framed_header(&title, width, color)];
    for row in rows {
        let safe = sanitize_terminal_text(row);
        for logical in safe.split('\n') {
            for part in wrap_line(logical, inner_width) {
                out.push(format!(
                    "{}{GUTTER}{}{GUTTER}{}",
                    paint(VERTICAL, BORDER, color),
                    pad_visible(&part, inner_width),
                    paint(VERTICAL, BORDER, color),
                ));
            }
        }
    }
    out.push(format!(
        "{}{}{}",
        paint(BOTTOM_LEFT, BORDER, color),
        paint(
            &HORIZONTAL.repeat(between_corners(width, BOTTOM_LEFT, BOTTOM_RIGHT)),
            BORDER,
            color
        ),
        paint(BOTTOM_RIGHT, BORDER, color),
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
    let inner = between_corners(width, VERTICAL, VERTICAL);
    // The right pane is what the left pane and the divider leave; one that
    // has no column beside its gutter is drawn as a single pane instead.
    let right_width = inner
        .checked_sub(pane + visible_len(SPLIT_TOP))
        .filter(|right| *right > visible_len(GUTTER));
    let Some(right_width) = right_width else {
        let flat = rows
            .iter()
            .map(|(left, right)| format!("{left} {right}"))
            .chain(std::iter::once(footer.to_string()))
            .collect::<Vec<_>>();
        return boxed(title, &flat, width, color);
    };
    // The title sits over the pane that can hold it: a long one would be
    // clipped to "Select model ro…" in the narrow brands column.
    let framed = format!(" {} ", sanitize_terminal_text(title));
    let title_left = visible_len(&framed) <= pane;
    let label_width = if title_left { pane } else { right_width };
    let label = clamp_visible(&framed, label_width);
    let head_pad = label_width.saturating_sub(visible_len(&label));
    let titled = format!(
        "{}{}",
        paint(&label, "bold", color),
        HORIZONTAL.repeat(head_pad)
    );
    let (left_head, right_head) = if title_left {
        (titled, HORIZONTAL.repeat(right_width))
    } else {
        (HORIZONTAL.repeat(pane), titled)
    };
    let mut out = vec![format!(
        "{}{}{}{}{}",
        paint(TOP_LEFT, BORDER, color),
        paint(&left_head, BORDER, color),
        paint(SPLIT_TOP, BORDER, color),
        paint(&right_head, BORDER, color),
        paint(TOP_RIGHT, BORDER, color),
    )];
    let gutter = visible_len(GUTTER);
    for (left, right) in rows {
        out.push(format!(
            "{}{GUTTER}{}{}{GUTTER}{}{}",
            paint(VERTICAL, BORDER, color),
            pad_visible(left, pane.saturating_sub(gutter)),
            paint(VERTICAL, BORDER, color),
            pad_visible(right, right_width.saturating_sub(gutter)),
            paint(VERTICAL, BORDER, color),
        ));
    }
    out.push(format!(
        "{}{}{}{}{}",
        paint(JOIN_LEFT, BORDER, color),
        paint(&HORIZONTAL.repeat(pane), BORDER, color),
        paint(SPLIT_BOTTOM, BORDER, color),
        paint(&HORIZONTAL.repeat(right_width), BORDER, color),
        paint(JOIN_RIGHT, BORDER, color),
    ));
    out.push(format!(
        "{}{GUTTER}{}{}",
        paint(VERTICAL, BORDER, color),
        pad_visible(footer, inner.saturating_sub(gutter)),
        paint(VERTICAL, BORDER, color),
    ));
    out.push(format!(
        "{}{}{}",
        paint(BOTTOM_LEFT, BORDER, color),
        paint(&HORIZONTAL.repeat(inner), BORDER, color),
        paint(BOTTOM_RIGHT, BORDER, color),
    ));
    out
}
