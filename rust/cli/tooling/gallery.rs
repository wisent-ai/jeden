//! `jeden gallery [--theme NAME|--all] [--color]` — render jeden's TUI
//! components (status line, message roles, tabbed picker, confirm panel,
//! welcome panel, QR code) with fixture data across bundled themes. Dev tool
//! for visual regression and theme authoring, mirroring omp's gallery.

use crate::tui::{self, ConfirmState, Message, PickerItem, PickerSpec, PickerState};
use crate::Args;

const PRESETS: &[&str] = &[
    "graphite-dark",
    "paper-light",
    "titanium",
    "nord",
    "color-blind",
];
const WIDTH: usize = 96;

/// One fixture pass under the currently effective theme.
fn fixture_lines(color: bool) -> Vec<String> {
    let mut lines = Vec::new();
    let status = tui::PromptStatus {
        cwd: "/Users/dev/project".into(),
        write_status: "ask".into(),
        command_status: "ask".into(),
        model: "codex/gpt-5.6-sol".into(),
        service_tier: "default".into(),
        branch: Some("main".into()),
        dirty_count: 2,
        context_percent: Some(6.4),
        context_limit: Some("272k".into()),
        cost: Some("$0.16".into()),
    };
    lines.extend(tui::compact_prompt(WIDTH, &status, "", false, color));
    lines.push(String::new());
    for (role, text) in [
        ("user", "Refactor the picker to support tabs"),
        (
            "assistant",
            "Done — Tab switches categories, search spans all of them.",
        ),
        ("system", "tools: read_file, edit_file"),
        ("error", "relay POST returned no seq"),
    ] {
        lines.extend(tui::message_block(&Message::new(role, text), WIDTH, color));
    }
    lines.push(String::new());
    let spec = PickerSpec::new(
        "Select model route",
        vec![
            PickerItem::action("any", "/model any")
                .detail("auto-select across available subscriptions")
                .badge("AUTO"),
            PickerItem::action("codex/gpt-5.6-sol", "/model codex/gpt-5.6-sol")
                .detail("context 272000 · output 128000 · tools · reasoning · free│ · 3.6s 19t/s")
                .badge("ACTIVE")
                .tab(1),
            PickerItem::action("codex/gpt-5.5", "/model codex/gpt-5.5")
                .detail("context 272000 · output 128000 · tools · reasoning · free│")
                .badge("AVAILABLE")
                .tab(1),
            PickerItem::action("kimi/kimi-for-coding", "/model kimi/kimi-for-coding")
                .detail("context 262144 · output 32000 · tools · reasoning · free│")
                .badge("AVAILABLE")
                .tab(2),
        ],
    )
    .with_tabs(vec!["All".into(), "codex".into(), "kimi".into()]);
    lines.extend(tui::picker_panel(&PickerState::new(spec), WIDTH, 14, color));
    lines.push(String::new());
    lines.extend(tui::confirm_panel(
        &ConfirmState::new(
            "Reset usage accounting".into(),
            "Clear all recorded events in .jeden/usage.json".into(),
            "/usage reset".into(),
            "en".into(),
        ),
        WIDTH,
        color,
    ));
    lines.push(String::new());
    lines.extend(tui::welcome_panel(
        WIDTH,
        "codex/gpt-5.6-sol",
        "/Users/dev/project",
        "ask",
        "ask",
        color,
    ));
    lines.push(String::new());
    if let Some(qr) = crate::qr::render("https://relay.example/room/demo#key=abc&role=view") {
        lines.extend(qr.lines().map(str::to_string));
    }
    lines
}

/// CLI `jeden gallery [--theme NAME|--all] [--color]`. Default renders one
/// pass under the currently effective theme; `--all` sweeps every preset.
pub(crate) fn gallery_command(args: &Args) -> Result<String, String> {
    let flag = |name: &str| args.positionals.iter().any(|part| part == name);
    let color = tui::stdout_supports_color() || flag("--color");
    let theme_arg = args
        .positionals
        .iter()
        .position(|part| part == "--theme")
        .and_then(|index| args.positionals.get(index + 1))
        .map(String::as_str);
    let selected: Vec<&str> = if flag("--all") {
        PRESETS.to_vec()
    } else if let Some(name) = theme_arg {
        vec![PRESETS
            .iter()
            .copied()
            .find(|preset| *preset == name)
            .ok_or_else(|| {
                format!(
                    "unknown theme `{name}`; bundled presets: {}",
                    PRESETS.join(", ")
                )
            })?]
    } else {
        Vec::new()
    };
    let saved_theme = std::env::var("JEDEN_THEME").ok();
    let mut out = Vec::new();
    if selected.is_empty() {
        out.extend(fixture_lines(color));
    } else {
        for preset in &selected {
            std::env::set_var("JEDEN_THEME", preset);
            out.push(format!("── theme: {preset} ──"));
            out.extend(fixture_lines(color));
            out.push(String::new());
        }
        match saved_theme {
            Some(value) => std::env::set_var("JEDEN_THEME", value),
            None => std::env::remove_var("JEDEN_THEME"),
        }
    }
    Ok(out.join("\n") + "\n")
}
