use super::{ConfirmState, PickerFocus, PickerItem, PickerState};
use crate::cli::i18n::tr;
use crate::tui::render::{boxed, boxed_split};
use crate::tui::text::{clamp_visible, paint, visible_len};

/// Marker column plus the dot in front of a category name.
const CATEGORY_MARKERS: &str = "❯ ● ";
/// The rules a two-pane frame draws across one row besides its panes: the
/// two outer borders and the divider.
const SPLIT_BORDERS: &str = "││┬";

/// The rows a two-pane frame spends on chrome rather than items. Undercount
/// them and the box grows past the terminal — the title is the first thing
/// lost.
enum Chrome {
    Query,
    Blank,
    DetailGap,
    Detail,
    Top,
    Rule,
    Footer,
    Bottom,
}

const CHROME: &[Chrome] = &[
    Chrome::Query,
    Chrome::Blank,
    Chrome::DetailGap,
    Chrome::Detail,
    Chrome::Top,
    Chrome::Rule,
    Chrome::Footer,
    Chrome::Bottom,
];

/// The items a tab shows: the first tab shows every item, each tab after it
/// the items filed under it.
fn items_in_tab(items: &[PickerItem], tab: usize) -> usize {
    if tab < usize::from(crate::tally::STEP) {
        return items.len();
    }
    items.iter().filter(|item| item.tab == tab).count()
}

/// Whether a tab can be used: as the spec marks it, or, where it marks none,
/// whether the tab holds any item.
fn reachable_tab(state: &PickerState, index: usize) -> bool {
    match state.spec.tab_marks.get(index) {
        Some(mark) => *mark,
        None => items_in_tab(&state.spec.items, index) != items_in_tab(&[], index),
    }
}

/// The brands column: cursor marker, ● for a category you can use and ○ for
/// one you cannot, the count right-aligned, and a rule where the reachable
/// group ends — the same reading order omp gives its provider pane.
fn category_rows(state: &PickerState, pane: usize, color: bool) -> Vec<String> {
    // Only the focused pane shows a live cursor, so it is never ambiguous
    // which column the arrow keys are driving.
    let focused = state.focus == PickerFocus::Categories;
    let mut rows = Vec::new();
    let mut ruled = false;
    for (index, name) in state.spec.tabs.iter().enumerate() {
        let reachable = reachable_tab(state, index);
        if !reachable && !ruled {
            rows.push(paint(&"─".repeat(pane), "dim", color));
            ruled = true;
        }
        let count = items_in_tab(&state.spec.items, index);
        let active = index == state.active_tab;
        let marker = if active && focused { "❯" } else { " " };
        let dot = if reachable { "●" } else { "○" };
        let head = format!("{marker} {dot} {name}");
        let tail = count.to_string();
        let pad = pane.saturating_sub(visible_len(&head) + visible_len(&tail));
        let row = clamp_visible(&format!("{head}{}{tail}", " ".repeat(pad)), pane);
        rows.push(match (active, reachable) {
            (true, _) => paint(&row, "bold", color),
            (false, true) => row,
            (false, false) => paint(&row, "dim", color),
        });
    }
    rows
}

/// `label … metrics`: the figures are pushed to the right edge so context,
/// throughput and price read as columns instead of drifting with the label.
fn item_row(item: &PickerItem, marker: &str, width: usize) -> String {
    // A row without a badge or detail shows only its label.
    let badge = match item.badge.as_deref() {
        Some(value) => format!(" [{value}]"),
        None => String::new(),
    };
    let head = format!("{marker} {}{badge}", item.label);
    if item.metrics.is_empty() {
        let detail = if item.detail.is_empty() {
            String::new()
        } else {
            format!(" — {}", item.detail)
        };
        return clamp_visible(&format!("{head}{detail}"), width);
    }
    let metrics = clamp_visible(&item.metrics, width);
    let head = clamp_visible(
        &head,
        width.saturating_sub(visible_len(&metrics) + visible_len(" ")),
    );
    let gap = width.saturating_sub(visible_len(&head) + visible_len(&metrics));
    format!("{head}{}{metrics}", " ".repeat(gap))
}

/// One item row styled for where it stands: disabled rows dim, the selected
/// row bold.
fn styled_item(row: String, disabled: bool, selected: bool, color: bool) -> String {
    match (disabled, selected) {
        (true, _) => paint(&row, "dim", color),
        (false, true) => paint(&row, "bold", color),
        (false, false) => row,
    }
}

pub(crate) fn picker_panel(
    state: &PickerState,
    width: usize,
    height: usize,
    color: bool,
) -> Vec<String> {
    let query_row = format!("{} {}", state.spec.prompt, state.query);
    let indices = state.filtered_indices();
    let visible_items = height
        .saturating_sub(CHROME.len())
        .max(usize::from(crate::tally::STEP));
    let start = state
        .selected
        .saturating_sub(visible_items.saturating_sub(usize::from(crate::tally::STEP)));
    // A tabbed picker is a two-pane view: brands on the left, the active
    // brand's items on the right, as wide as its widest brand and count. The
    // frame keeps one space of padding inside each cell, so the width budgets
    // for it or the count gets clipped to an ellipsis. An untabbed picker
    // stays a flat list — a single column of rows has no brands beside it.
    let widest_brand = state
        .spec
        .tabs
        .iter()
        .enumerate()
        .map(|(index, name)| {
            visible_len(CATEGORY_MARKERS)
                + visible_len(name)
                + visible_len("  ")
                + items_in_tab(&state.spec.items, index).to_string().len()
        })
        .max();
    let Some(pane) = widest_brand else {
        let mut rows = vec![query_row];
        if indices.is_empty() {
            rows.push(paint(&state.spec.empty_message, "dim", color));
        } else {
            for (position, index) in indices
                .into_iter()
                .enumerate()
                .skip(start)
                .take(visible_items)
            {
                let item = &state.spec.items[index];
                let selected = position == state.selected;
                let marker = if selected { "›" } else { " " };
                let row = item_row(item, marker, width.saturating_sub(visible_len(CATEGORY_MARKERS)));
                rows.push(styled_item(row, item.disabled, selected, color));
            }
        }
        rows.push(paint(tr(&state.spec.lang, "picker.footer"), "dim", color));
        return boxed(&state.spec.title, &rows, width, color);
    };

    // `pane` is the frame column; the cell inside it is one gutter narrower,
    // and a row built to the wider figure loses its count to the ellipsis.
    let mut categories = category_rows(state, pane.saturating_sub(visible_len(" ")), color);
    let right_width = width.saturating_sub(pane + visible_len(SPLIT_BORDERS));
    let mut right = vec![
        paint(&clamp_visible(&query_row, right_width), "dim", color),
        String::new(),
    ];
    if indices.is_empty() {
        right.push(paint(&state.spec.empty_message, "dim", color));
    } else {
        for (position, index) in indices
            .iter()
            .copied()
            .enumerate()
            .skip(start)
            .take(visible_items)
        {
            let item = &state.spec.items[index];
            let selected = position == state.selected;
            let on_cursor = selected && state.focus == PickerFocus::Items;
            let marker = if on_cursor { "›" } else { " " };
            let row = item_row(item, marker, right_width);
            right.push(styled_item(row, item.disabled, selected, color));
        }
    }
    // Bottom of the item pane: what the cursor is actually on, spelled out —
    // the row itself only has room for the figures. An empty list has
    // nothing under the cursor to spell out.
    if let Some(index) = indices.get(state.selected) {
        let detail = &state.spec.items[*index].detail;
        if !detail.is_empty() {
            right.push(String::new());
            right.push(paint(&clamp_visible(detail, right_width), "dim", color));
        }
    }
    // The shorter pane is padded with blank cells to the taller one's height.
    let rows = categories.len().max(right.len());
    categories.resize(rows, String::new());
    right.resize(rows, String::new());
    let paired = categories.into_iter().zip(right).collect::<Vec<_>>();
    let footer = tr(&state.spec.lang, "picker.footer.panes");
    boxed_split(
        &state.spec.title,
        pane,
        &paired,
        &paint(footer, "dim", color),
        width,
        color,
    )
}

pub(crate) fn confirm_panel(state: &ConfirmState, width: usize, color: bool) -> Vec<String> {
    let cancel_marker = if state.confirmed { " " } else { "›" };
    let confirm_marker = if state.confirmed { "›" } else { " " };
    let mut rows = vec![
        paint(
            "This action changes or removes persisted state.",
            "yellow",
            color,
        ),
        state.label.clone(),
    ];
    if !state.detail.trim().is_empty() {
        rows.push(state.detail.clone());
    }
    rows.push(String::new());
    rows.push(format!("{cancel_marker} Cancel"));
    let confirm_style = match state.confirmed {
        true => "red",
        false => "dim",
    };
    rows.push(paint(&format!("{confirm_marker} Confirm"), confirm_style, color));
    rows.push(paint("←→ choose  Enter confirm  Esc cancel", "dim", color));
    boxed(tr(&state.lang, "view.confirm.title"), &rows, width, color)
}
