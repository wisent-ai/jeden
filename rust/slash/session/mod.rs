use serde_json::{json, Value};
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};

use crate::slash::common::{file_url, now_text, read_json_value, resolve_cwd_path, split_args};
use crate::slash::state::mode_state_path;
use crate::slash::SlashContext;
use crate::tui::{PickerItem, PickerSpec};

pub(crate) mod clipboard;
pub(crate) mod collab;

pub(super) fn collab_picker(context: &SlashContext<'_>) -> PickerSpec {
    collab::build_collab_picker(context)
}

pub(super) fn join_picker(context: &SlashContext<'_>) -> PickerSpec {
    collab::build_join_picker(context)
}

pub(super) fn leave_picker(context: &SlashContext<'_>) -> PickerSpec {
    collab::build_leave_picker(context)
}

pub(super) fn copy_picker() -> PickerSpec {
    clipboard::build_copy_picker()
}

mod commands;

use commands::{slash_session_export, slash_session_text};
pub(super) use commands::{
    dump_picker, export_picker, jobs_picker, omfg_picker, share_picker, tan_picker,
};
pub(crate) use commands::{handle_jobs, handle_omfg, handle_share, handle_tan};

pub(super) fn slash_session_dir(
    context: &SlashContext<'_>,
    id_or_path: &str,
) -> Result<PathBuf, String> {
    let target = if id_or_path.trim().is_empty() {
        read_json_value(&mode_state_path(context.cwd))
            .get("lastSessionPath")
            .and_then(Value::as_str)
            .map(str::to_string)
            .ok_or("No current Rust session is recorded yet; pass a session id or path.")?
    } else {
        id_or_path.trim().to_string()
    };
    let raw_path = PathBuf::from(&target);
    let path = if target.contains('/') {
        if raw_path.is_absolute() {
            raw_path
        } else {
            context.cwd.join(raw_path)
        }
    } else {
        context.session_root.join(target)
    };
    if !path.exists() {
        return Err(format!("session not found: {}", path.display()));
    }
    Ok(path)
}

pub(super) fn slash_session_value(
    context: &SlashContext<'_>,
    id_or_path: &str,
) -> Result<Value, String> {
    let dir = slash_session_dir(context, id_or_path)?;
    crate::cli::sessions::read_session_value(&slash_command_path(&dir))
}

pub(super) fn slash_command_path(path: &Path) -> String {
    path.to_string_lossy()
        .replace(std::path::MAIN_SEPARATOR, "/")
}

pub(crate) fn handle_dump(args: &str, context: &SlashContext<'_>) -> Result<String, String> {
    Ok(slash_session_text(&slash_session_value(
        context,
        args.trim(),
    )?))
}

pub(crate) fn handle_export(args: &str, context: &SlashContext<'_>) -> Result<String, String> {
    let argv = split_args(args);
    let mut id = String::new();
    let mut format = "json".to_string();
    let mut output: Option<String> = None;
    for arg in argv {
        if arg == "--html" {
            format = "html".into();
        } else if arg == "--markdown" || arg == "--md" {
            format = "markdown".into();
        } else if id.is_empty()
            && !arg.starts_with("--")
            && slash_session_dir(context, &arg).is_ok()
        {
            id = arg;
        } else {
            output = Some(arg);
        }
    }
    let payload = slash_session_export(&slash_session_value(context, &id)?, &format)?;
    if let Some(path) = output {
        let target = resolve_cwd_path(context.cwd, &path);
        if let Some(parent) = target.parent() {
            fs::create_dir_all(parent).map_err(|e| e.to_string())?;
        }
        fs::write(&target, &payload).map_err(|e| e.to_string())?;
        Ok(target.display().to_string())
    } else {
        Ok(payload)
    }
}
