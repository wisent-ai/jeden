//! The rows the terminal shows when one of these session commands is opened
//! without arguments.
//!
//! Split out of `slash/session/mod.rs`, which had grown past the module line
//! cap.

use crate::slash::session::{slash_command_path, slash_session_dir};
use crate::slash::SlashContext;
use crate::tui::{PickerItem, PickerSpec};

fn current_session_picker_detail(context: &SlashContext<'_>) -> String {
    match slash_session_dir(context, "") {
        Ok(path) => {
            let id = path
                .file_name()
                .map(|value| value.to_string_lossy())
                .unwrap_or_else(|| path.to_string_lossy());
            format!("Current session {id} at {}", path.display())
        }
        Err(error) => error,
    }
}


fn current_session_command(context: &SlashContext<'_>, command: &str) -> Option<String> {
    slash_session_dir(context, "")
        .ok()
        .map(|path| format!("{command} {}", slash_command_path(&path)))
}

pub(crate) fn dump_picker(context: &SlashContext<'_>) -> PickerSpec {
    let command = current_session_command(context, "/dump");
    PickerSpec::new(
        "Dump session",
        vec![PickerItem::action(
            "Print current session transcript",
            command.clone().unwrap_or_default(),
        )
        .detail(current_session_picker_detail(context))
        .badge("text")
        .disabled(command.is_none())],
    )
}

pub(crate) fn export_picker(context: &SlashContext<'_>) -> PickerSpec {
    let session = slash_session_dir(context, "").ok();
    let detail = current_session_picker_detail(context);
    let command = |prefix: &str| {
        session
            .as_ref()
            .map(|path| {
                let path = slash_command_path(path)
                    .replace('\\', "\\\\")
                    .replace('"', "\\\"");
                format!("{prefix} \"{path}\"")
            })
            .unwrap_or_default()
    };
    PickerSpec::new(
        "Export session",
        vec![
            PickerItem::action("Print JSON export", command("/export"))
                .detail(&detail)
                .badge("JSON")
                .disabled(session.is_none()),
            PickerItem::action("Print Markdown export", command("/export --markdown"))
                .detail(&detail)
                .badge("Markdown")
                .disabled(session.is_none()),
            PickerItem::action("Print HTML export", command("/export --html"))
                .detail(detail)
                .badge("HTML")
                .disabled(session.is_none()),
        ],
    )
}

pub(crate) fn share_picker(context: &SlashContext<'_>) -> PickerSpec {
    let available = slash_session_dir(context, "").is_ok();
    let detail = current_session_picker_detail(context);
    PickerSpec::new(
        "Share session",
        vec![
            PickerItem::action("Create encrypted share bundle", "/share bundle")
                .detail(&detail)
                .badge("writes artifact")
                .disabled(!available),
            PickerItem::action("Create bundle and copy share URL", "/share --copy")
                .detail(detail)
                .badge("writes artifact + clipboard")
                .disabled(!available),
        ],
    )
}

pub(crate) fn tan_picker(context: &SlashContext<'_>) -> PickerSpec {
    PickerSpec::new(
        "Start background agent job",
        vec![PickerItem::action("Enter background work", "/tan ")
            .detail(format!(
                "Edit the work request before submitting. {}",
                current_session_picker_detail(context)
            ))
            .badge("INPUT")
            .prefill()],
    )
}

pub(crate) fn omfg_picker(context: &SlashContext<'_>) -> PickerSpec {
    PickerSpec::new(
        "Forge a local rule",
        vec![PickerItem::action("Describe the local rule", "/omfg ")
            .detail(format!(
                "Edit the complaint before submitting. Rules file: {}",
                context.cwd.join(".jeden/rules.jsonl").display()
            ))
            .badge("INPUT")
            .prefill()],
    )
}
