use super::render::boxed;
use super::text::{clamp_visible, paint};
use super::{ConfirmState, PickerState};
use crate::cli::i18n::tr;

pub(crate) fn picker_panel(
    state: &PickerState,
    width: usize,
    height: usize,
    color: bool,
) -> Vec<String> {
    let mut rows = vec![format!("{} {}", state.spec.prompt, state.query)];
    let has_tabs = !state.spec.tabs.is_empty();
    if has_tabs {
        let bar = state
            .spec
            .tabs
            .iter()
            .enumerate()
            .map(|(index, name)| {
                if index == state.active_tab {
                    format!("[{name}]")
                } else {
                    format!(" {name} ")
                }
            })
            .collect::<Vec<_>>()
            .join(" ");
        rows.push(paint(&bar, "magenta", color));
    }
    let indices = state.filtered_indices();
    let chrome_rows = ["query", "footer", "top", "bottom", "prompt"].len() + usize::from(has_tabs);
    let visible_items = height.saturating_sub(chrome_rows).max(usize::from(true));
    let start = state
        .selected
        .saturating_sub(visible_items.saturating_sub(usize::from(true)));
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
            let row = clamp_visible(
                &format!("{marker} {}{badge}{detail}", item.label),
                width.saturating_sub(4),
            );
            rows.push(if item.disabled {
                paint(&row, "dim", color)
            } else if position == state.selected {
                paint(&row, "bold", color)
            } else {
                row
            });
        }
    }
    let footer = if has_tabs {
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
