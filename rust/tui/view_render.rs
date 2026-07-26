use super::render::boxed;
use super::text::{clamp_visible, paint, visible_len};
use super::{ConfirmState, PickerItem, PickerState};
use crate::cli::i18n::tr;

/// Marker column plus the dot in front of a category name.
const CATEGORY_MARKERS: &str = "❯ ● ";
/// Divider drawn between the category pane and the item pane.
const PANE_DIVIDER: &str = " │ ";

fn items_in_tab(items: &[PickerItem], tab: usize) -> usize {
    if tab == usize::default() {
        return items.len();
    }
    items.iter().filter(|item| item.tab == tab).count()
}

/// Category rows for the left pane: a filled dot when the category has items,
/// a hollow one when it is empty, the cursor marker on the active tab, and
/// the item count right-aligned. This is the brands column; the pane to its
/// right lists what the active brand offers.
fn category_rows(state: &PickerState, pane: usize, color: bool) -> Vec<String> {
    state
        .spec
        .tabs
        .iter()
        .enumerate()
        .map(|(index, name)| {
            let count = items_in_tab(&state.spec.items, index);
            let filled = count != usize::default();
            let marker = if index == state.active_tab { "❯" } else { " " };
            let dot = if filled { "●" } else { "○" };
            let head = format!("{marker} {dot} {name}");
            let tail = count.to_string();
            let pad = pane.saturating_sub(visible_len(&head) + visible_len(&tail));
            let row = format!("{head}{}{tail}", " ".repeat(pad));
            if index == state.active_tab {
                paint(&row, "bold", color)
            } else if filled {
                row
            } else {
                paint(&row, "dim", color)
            }
        })
        .collect()
}

pub(crate) fn picker_panel(
    state: &PickerState,
    width: usize,
    height: usize,
    color: bool,
) -> Vec<String> {
    let mut rows = vec![format!("{} {}", state.spec.prompt, state.query)];
    // A tabbed picker is a two-pane view: categories on the left, the active
    // category's items on the right. Untabbed pickers stay a flat list — a
    // single column of rows has no brands to put beside it.
    let two_pane = !state.spec.tabs.is_empty();
    let pane = state
        .spec
        .tabs
        .iter()
        .enumerate()
        .map(|(index, name)| {
            visible_len(CATEGORY_MARKERS)
                + visible_len(name)
                + visible_len(" ")
                + items_in_tab(&state.spec.items, index).to_string().len()
        })
        .max()
        .unwrap_or_default();
    let categories = if two_pane {
        category_rows(state, pane, color)
    } else {
        Vec::new()
    };
    let indices = state.filtered_indices();
    let chrome_rows = ["query", "footer", "top", "bottom", "prompt"].len();
    let visible_items = height
        .saturating_sub(chrome_rows)
        .max(usize::from(true));
    let start = state
        .selected
        .saturating_sub(visible_items.saturating_sub(usize::from(true)));
    let item_width = width.saturating_sub(if two_pane {
        pane + visible_len(PANE_DIVIDER) + visible_len(CATEGORY_MARKERS)
    } else {
        visible_len(CATEGORY_MARKERS)
    });
    let mut item_rows = Vec::new();
    if indices.is_empty() {
        item_rows.push(paint(&state.spec.empty_message, "dim", color));
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
            let badge = item
                .badge
                .as_deref()
                .map(|value| format!(" [{}]", value))
                .unwrap_or_default();
            let detail = if item.detail.is_empty() {
                String::new()
            } else {
                format!(" — {}", item.detail)
            };
            // One visible line per item, always: the scroll math above counts
            // items, so wrapped rows would push the box past the viewport and
            // hide the title/prompt/tab bar. Overflow gets an ellipsis.
            let row = clamp_visible(&format!("{marker} {}{badge}{detail}", item.label), item_width);
            item_rows.push(if item.disabled {
                paint(&row, "dim", color)
            } else if position == state.selected {
                paint(&row, "bold", color)
            } else {
                row
            });
        }
    }
    if two_pane {
        let pane_rows = categories.len().max(item_rows.len());
        for offset in usize::default()..pane_rows {
            let left = categories.get(offset).cloned().unwrap_or_default();
            let pad = pane.saturating_sub(visible_len(&left));
            let right = item_rows.get(offset).cloned().unwrap_or_default();
            rows.push(format!("{left}{}{PANE_DIVIDER}{right}", " ".repeat(pad)));
        }
    } else {
        rows.extend(item_rows);
    }
    let footer = if two_pane {
        format!(
            "{}  {}",
            tr(&state.spec.lang, "picker.footer"),
            tr(&state.spec.lang, "picker.footer.tabs")
        )
    } else {
        tr(&state.spec.lang, "picker.footer").to_string()
    };
    rows.push(paint(&footer, "dim", color));
    boxed(&state.spec.title, &rows, width, color)
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
