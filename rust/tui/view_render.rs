use super::render::{boxed, boxed_split};
use super::text::{clamp_visible, paint, visible_len};
use super::{ConfirmState, PickerFocus, PickerItem, PickerState};
use crate::cli::i18n::tr;

/// Marker column plus the dot in front of a category name.
const CATEGORY_MARKERS: &str = "❯ ● ";

fn items_in_tab(items: &[PickerItem], tab: usize) -> usize {
    if tab == usize::default() {
        return items.len();
    }
    items.iter().filter(|item| item.tab == tab).count()
}

fn reachable_tab(state: &PickerState, index: usize) -> bool {
    state
        .spec
        .tab_marks
        .get(index)
        .copied()
        .unwrap_or(items_in_tab(&state.spec.items, index) != usize::default())
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
        rows.push(if active {
            paint(&row, "bold", color)
        } else if reachable {
            row
        } else {
            paint(&row, "dim", color)
        });
    }
    rows
}

/// `label … metrics`: the figures are pushed to the right edge so context,
/// throughput and price read as columns instead of drifting with the label.
fn item_row(item: &PickerItem, marker: &str, width: usize) -> String {
    let badge = item
        .badge
        .as_deref()
        .map(|value| format!(" [{}]", value))
        .unwrap_or_default();
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

pub(crate) fn picker_panel(
    state: &PickerState,
    width: usize,
    height: usize,
    color: bool,
) -> Vec<String> {
    // A tabbed picker is a two-pane view: brands on the left, the active
    // brand's items on the right. Untabbed pickers stay a flat list — a
    // single column of rows has no brands to put beside it.
    let two_pane = !state.spec.tabs.is_empty();
    let query_row = format!("{} {}", state.spec.prompt, state.query);
    let indices = state.filtered_indices();
    let selected_detail = indices
        .get(state.selected)
        .map(|index| state.spec.items[*index].detail.clone())
        .unwrap_or_default();
    // Chrome inside a two-pane frame: the search row, the blank under it, the
    // blank and detail line at the bottom, the top border, the rule above the
    // footer, the footer itself and the bottom border. Undercount this and
    // the box grows past the terminal — the title is the first thing lost.
    let chrome_rows = [
        "query",
        "blank",
        "detail-gap",
        "detail",
        "top",
        "rule",
        "footer",
        "bottom",
    ]
    .len();
    let visible_items = height.saturating_sub(chrome_rows).max(usize::from(true));
    let start = state
        .selected
        .saturating_sub(visible_items.saturating_sub(usize::from(true)));
    if !two_pane {
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
                let marker = if position == state.selected {
                    "›"
                } else {
                    " "
                };
                let row = item_row(item, marker, width.saturating_sub(CATEGORY_MARKERS.len()));
                rows.push(if item.disabled {
                    paint(&row, "dim", color)
                } else if position == state.selected {
                    paint(&row, "bold", color)
                } else {
                    row
                });
            }
        }
        rows.push(paint(tr(&state.spec.lang, "picker.footer"), "dim", color));
        return boxed(&state.spec.title, &rows, width, color);
    }

    let pane = state
        .spec
        .tabs
        .iter()
        .enumerate()
        .map(|(index, name)| {
            // The frame keeps one space of padding inside each cell, so the
            // widest brand plus its count has to budget for it or the count
            // gets clipped to an ellipsis.
            visible_len(CATEGORY_MARKERS)
                + visible_len(name)
                + visible_len("  ")
                + items_in_tab(&state.spec.items, index).to_string().len()
        })
        .max()
        .unwrap_or_default();
    // `pane` is the frame column; the cell inside it is one gutter narrower,
    // and a row built to the wider figure loses its count to the ellipsis.
    let categories = category_rows(state, pane.saturating_sub(visible_len(" ")), color);
    let right_width = width.saturating_sub(pane + "││┬│".len());
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
            let on_cursor = position == state.selected && state.focus == PickerFocus::Items;
            let marker = if on_cursor { "›" } else { " " };
            let row = item_row(item, marker, right_width);
            right.push(if item.disabled {
                paint(&row, "dim", color)
            } else if position == state.selected {
                paint(&row, "bold", color)
            } else {
                row
            });
        }
    }
    // Bottom of the item pane: what the cursor is actually on, spelled out —
    // the row itself only has room for the figures.
    if !selected_detail.is_empty() {
        right.push(String::new());
        right.push(paint(
            &clamp_visible(&selected_detail, right_width),
            "dim",
            color,
        ));
    }
    let rows = usize::default()..categories.len().max(right.len());
    let paired = rows
        .map(|offset| {
            (
                categories.get(offset).cloned().unwrap_or_default(),
                right.get(offset).cloned().unwrap_or_default(),
            )
        })
        .collect::<Vec<_>>();
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
    rows.push(paint(
        &format!("{confirm_marker} Confirm"),
        if state.confirmed { "red" } else { "dim" },
        color,
    ));
    rows.push(paint("←→ choose  Enter confirm  Esc cancel", "dim", color));
    boxed(tr(&state.lang, "view.confirm.title"), &rows, width, color)
}
