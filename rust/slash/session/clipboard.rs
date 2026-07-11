use std::env;
use std::io::Write;
use std::process::{Command, Stdio};

use crate::slash::SlashContext;
use crate::tui::{PickerItem, PickerSpec};

fn clipboard_candidates() -> Vec<(&'static str, Vec<&'static str>)> {
    match env::consts::OS {
        "macos" => vec![("pbcopy", vec![])],
        "windows" => vec![
            (
                "powershell.exe",
                vec![
                    "-NoProfile",
                    "-NonInteractive",
                    "-Command",
                    "Set-Clipboard -Value ([Console]::In.ReadToEnd())",
                ],
            ),
            ("clip.exe", vec![]),
        ],
        _ => vec![
            ("wl-copy", vec![]),
            ("xclip", vec!["-selection", "clipboard"]),
            ("xsel", vec!["--clipboard", "--input"]),
        ],
    }
}

pub(super) fn write_clipboard(payload: &str) -> Result<String, String> {
    let mut last_error = "no clipboard command was attempted".to_string();
    for (command, args) in clipboard_candidates() {
        match Command::new(command)
            .args(args)
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(Stdio::piped())
            .spawn()
        {
            Ok(mut child) => {
                if let Some(stdin) = child.stdin.as_mut() {
                    if let Err(error) = stdin.write_all(payload.as_bytes()) {
                        last_error = error.to_string();
                        let _ = child.kill();
                        continue;
                    }
                }
                match child.wait_with_output() {
                    Ok(output) if output.status.success() => return Ok(command.to_string()),
                    Ok(output) => {
                        let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
                        last_error = if stderr.is_empty() {
                            format!("{command} exited with {}", output.status)
                        } else {
                            stderr
                        };
                    }
                    Err(error) => last_error = error.to_string(),
                }
            }
            Err(error) => last_error = error.to_string(),
        }
    }
    Err(last_error)
}

pub(super) fn build_copy_picker() -> PickerSpec {
    PickerSpec::new(
        "Copy text",
        vec![PickerItem::action("Enter text to copy", "/copy ")
            .detail("Edit the text in the main prompt before submitting")
            .badge("INPUT")
            .prefill()],
    )
}

pub(crate) fn handle_copy(args: &str, _context: &SlashContext<'_>) -> Result<String, String> {
    let payload = args.trim();
    if payload.is_empty() {
        return Err("/copy without text requires a live session recorder; pass text explicitly with /copy <text> in interactive Jeden.".into());
    }
    let command = write_clipboard(payload)?;
    Ok(format!(
        "Copied provided text to the OS clipboard with {}.",
        command
    ))
}
