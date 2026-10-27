//! `jeden gallery [--theme NAME|--all] [--color]` — render jeden's TUI
//! components (status line, message roles, tabbed picker, confirm panel,
//! welcome panel, QR code) across the compiled-in themes. Dev tool for visual
//! regression and theme authoring, mirroring omp's gallery. The panels are
//! drawn at the terminal's own size and the status line shows the working
//! directory's real git branch and uncommitted count, so the gallery runs in
//! a terminal and refuses elsewhere naming why.

use crate::cli::run::interactive::status::git_prompt;
use crate::tui::{self, ConfirmState, Message, PickerItem, PickerSpec, PickerState};
use crate::Args;

/// The fixture picker's routes: label, command, detail, badge. A route sits
/// on its provider's tab, the part of its label before the slash; one with
/// no provider sits on the first tab.
const ROUTES: &[(&str, &str, &str, &str)] = &[
    (
        "any",
        "/model any",
        "auto-select across available subscriptions",
        "AUTO",
    ),
    (
        "codex/gpt-5.6-sol",
        "/model codex/gpt-5.6-sol",
        "context 272000 · output 128000 · tools · reasoning · free│ · 3.6s 19t/s",
        "ACTIVE",
    ),
    (
        "codex/gpt-5.5",
        "/model codex/gpt-5.5",
        "context 272000 · output 128000 · tools · reasoning · free│",
        "AVAILABLE",
    ),
    (
        "kimi/kimi-for-coding",
        "/model kimi/kimi-for-coding",
        "context 262144 · output 32000 · tools · reasoning · free│",
        "AVAILABLE",
    ),
];

/// The fixture picker: its routes on their providers' tabs, after the
/// first tab that lists every route.
fn route_picker() -> PickerSpec {
    let mut tabs = vec!["All".to_string()];
    let mut items = Vec::new();
    for (label, command, detail, badge) in ROUTES {
        let mut item = PickerItem::action(*label, *command)
            .detail(*detail)
            .badge(*badge);
        if let Some((provider, _)) = label.split_once('/') {
            let tab = match tabs.iter().position(|tab| tab == provider) {
                Some(tab) => tab,
                None => {
                    let tab = tabs.len();
                    tabs.push(provider.to_string());
                    tab
                }
            };
            item = item.tab(tab);
        }
        items.push(item);
    }
    PickerSpec::new("Select model route", items).with_tabs(tabs)
}

/// One pass under the currently effective theme, `width` columns wide with
/// the picker `height` rows tall.
fn fixture_lines(args: &Args, width: usize, height: usize, color: bool) -> Vec<String> {
    let cwd = args.cwd.display().to_string();
    let git = git_prompt(&args.cwd);
    let mut lines = Vec::new();
    let status = tui::PromptStatus {
        cwd: cwd.clone(),
        write_status: "ask".into(),
        command_status: "ask".into(),
        model: "codex/gpt-5.6-sol".into(),
        service_tier: "default".into(),
        branch: git.branch,
        dirty_count: git.dirty_count,
        context_percent: None,
        context_limit: Some("272k".into()),
        cost: Some("$0.16".into()),
    };
    lines.extend(tui::compact_prompt(width, &status, "", false, color));
    lines.push(String::new());
    for (role, text) in [
        (tui::Role::User, "Refactor the picker to support tabs"),
        (
            tui::Role::Assistant,
            "Done — Tab switches categories, search spans all of them.",
        ),
        (tui::Role::System, "tools: read_file, edit_file"),
        (tui::Role::Error, "relay POST returned no seq"),
    ] {
        lines.extend(tui::message_block(&Message::new(role, text), width, color));
    }
    lines.push(String::new());
    lines.extend(tui::picker_panel(
        &PickerState::new(route_picker()),
        width,
        height,
        color,
    ));
    lines.push(String::new());
    lines.extend(tui::confirm_panel(
        &ConfirmState::new(
            "Reset usage accounting".into(),
            "Clear all recorded events in .jeden/usage.json".into(),
            "/usage reset".into(),
            "en".into(),
        ),
        width,
        color,
    ));
    lines.push(String::new());
    lines.extend(tui::welcome_panel(
        width,
        "codex/gpt-5.6-sol",
        &cwd,
        "ask",
        "ask",
        color,
    ));
    lines.push(String::new());
    match crate::qr::render("https://relay.example/room/demo#key=abc&role=view") {
        Ok(qr) => lines.extend(qr.lines().map(str::to_string)),
        Err(cause) => lines.push(format!(
            "(No QR code for this link: {cause}; open it as text.)"
        )),
    }
    lines
}

/// CLI `jeden gallery [--theme NAME|--all] [--color]`. Default renders one
/// pass under the currently effective theme; `--all` sweeps every
/// compiled-in preset.
pub(crate) fn gallery_command(args: &Args) -> Result<String, String> {
    let flag = |name: &str| args.positionals.iter().any(|part| part == name);
    let color = tui::stdout_supports_color() || flag("--color");
    let mut after_theme = args
        .positionals
        .iter()
        .skip_while(|part| part.as_str() != "--theme");
    let theme_arg = match after_theme.next() {
        Some(_flag) => Some(
            after_theme
                .next()
                .ok_or("--theme needs the name of a preset")?
                .as_str(),
        ),
        None => None,
    };
    let presets = || tui::theme::preset_names();
    let selected: Vec<&str> = if flag("--all") {
        presets().collect()
    } else if let Some(name) = theme_arg {
        vec![presets().find(|preset| *preset == name).ok_or_else(|| {
            format!(
                "unknown theme `{name}`; bundled presets: {}",
                presets().collect::<Vec<_>>().join(", ")
            )
        })?]
    } else {
        Vec::new()
    };
    let (width, height) = tui::terminal_dimensions().map_err(|error| {
        format!("jeden gallery draws at the terminal's size, so it runs in a terminal: {error}")
    })?;
    let saved_theme = std::env::var_os("JEDEN_THEME");
    let mut out = Vec::new();
    if selected.is_empty() {
        out.extend(fixture_lines(args, width, height, color));
    } else {
        for preset in &selected {
            std::env::set_var("JEDEN_THEME", preset);
            out.push(format!("── theme: {preset} ──"));
            out.extend(fixture_lines(args, width, height, color));
            out.push(String::new());
        }
        match saved_theme {
            Some(value) => std::env::set_var("JEDEN_THEME", value),
            None => std::env::remove_var("JEDEN_THEME"),
        }
    }
    Ok(out.join("\n") + "\n")
}
