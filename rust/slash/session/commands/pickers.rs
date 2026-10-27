//! The rows the terminal shows when one of these session commands is opened
//! without arguments. Without a current session a row is shown disabled,
//! with the reason there is none as its detail, and carries no command.

use crate::slash::session::{slash_command_path, slash_session_dir};
use crate::slash::SlashContext;
use crate::tui::{PickerItem, PickerSpec};

fn current_session_picker_detail(context: &SlashContext<'_>) -> String {
    match slash_session_dir(context, "") {
        Ok(path) => {
            let id = match path.file_name() {
                Some(name) => name.to_string_lossy(),
                None => path.to_string_lossy(),
            };
            format!("Current session {id} at {}", path.display())
        }
        Err(error) => error,
    }
}

/// `command` addressed to the current session, or none when there is no
/// session; why there is none is the detail of the row it would run from.
fn current_session_command(context: &SlashContext<'_>, command: &str) -> Option<String> {
    match slash_session_dir(context, "") {
        Ok(path) => Some(format!("{command} {}", slash_command_path(&path))),
        Err(_said_in_the_detail) => None,
    }
}

/// A row that runs `command`, or a disabled row with no command to run.
fn session_row(label: &str, command: Option<String>) -> PickerItem {
    match command {
        Some(command) => PickerItem::action(label, command),
        None => PickerItem::action(label, String::new()).disabled(true),
    }
}

pub(crate) fn dump_picker(context: &SlashContext<'_>) -> PickerSpec {
    let command = current_session_command(context, "/dump");
    PickerSpec::new(
        "Dump session",
        vec![session_row("Print current session transcript", command)
            .detail(current_session_picker_detail(context))
            .badge("text")],
    )
}

pub(crate) fn export_picker(context: &SlashContext<'_>) -> PickerSpec {
    let session = match slash_session_dir(context, "") {
        Ok(path) => Some(path),
        Err(_said_in_the_detail) => None,
    };
    let detail = current_session_picker_detail(context);
    let command = |prefix: &str| {
        session.as_ref().map(|path| {
            let path = slash_command_path(path)
                .replace('\\', "\\\\")
                .replace('"', "\\\"");
            format!("{prefix} \"{path}\"")
        })
    };
    PickerSpec::new(
        "Export session",
        vec![
            session_row("Print JSON export", command("/export"))
                .detail(&detail)
                .badge("JSON"),
            session_row("Print Markdown export", command("/export --markdown"))
                .detail(&detail)
                .badge("Markdown"),
            session_row("Print HTML export", command("/export --html"))
                .detail(detail)
                .badge("HTML"),
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
