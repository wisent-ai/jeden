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

/// How the same clipboard is read back, in the order matching the writers
/// above. A writer that exits zero has not proved anything: the hand-off is
/// only real if the clipboard holds the payload afterwards.
fn clipboard_readers() -> Vec<(&'static str, Vec<&'static str>)> {
    match env::consts::OS {
        "macos" => vec![("pbpaste", vec![])],
        "windows" => vec![(
            "powershell.exe",
            vec![
                "-NoProfile",
                "-NonInteractive",
                "-Command",
                "Get-Clipboard -Raw",
            ],
        )],
        _ => vec![
            ("wl-paste", vec!["--no-newline"]),
            ("xclip", vec!["-selection", "clipboard", "-o"]),
            ("xsel", vec!["--clipboard", "--output"]),
        ],
    }
}

/// What a command that answered with a failure said: its stderr, or its exit
/// status when it wrote none.
fn refused(command: &str, output: &std::process::Output) -> String {
    let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
    if stderr.is_empty() {
        format!("{command} exited with {}", output.status)
    } else {
        format!("{command}: {stderr}")
    }
}

/// Every clipboard command tried and what each answered, so a refusal names
/// the one that is missing as well as the one that failed.
fn none_answered(what: &str, attempts: &[String]) -> String {
    if attempts.is_empty() {
        format!("no clipboard {what} exists for {}", env::consts::OS)
    } else {
        format!("no clipboard {what} answered: {}", attempts.join("; "))
    }
}

/// What the clipboard holds right now, and which command answered.
pub(crate) fn read_clipboard() -> Result<(String, String), String> {
    let mut attempts = Vec::new();
    for (command, args) in clipboard_readers() {
        let read = stado_wait::blocking(stado_wait::Kind::Process, "the clipboard reader", command, || {
            Command::new(command).args(&args).output()
        });
        match read {
            Ok(output) if output.status.success() => {
                return Ok((
                    String::from_utf8_lossy(&output.stdout).to_string(),
                    command.to_string(),
                ))
            }
            Ok(output) => attempts.push(refused(command, &output)),
            Err(error) => attempts.push(format!("{command}: {error}")),
        }
    }
    Err(none_answered("reader", &attempts))
}

/// Whether a writer keeps the trailing newline is the writer's business; the
/// payload's own text is what has to survive the round trip.
pub(crate) fn same_payload(left: &str, right: &str) -> bool {
    left.replace("\r\n", "\n").trim_end_matches('\n')
        == right.replace("\r\n", "\n").trim_end_matches('\n')
}

pub(crate) fn write_clipboard(payload: &str) -> Result<String, String> {
    let mut attempts = Vec::new();
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
                        attempts.push(match child.kill() {
                            Ok(()) => format!("{command}: {error}"),
                            Err(kill) => {
                                format!("{command}: {error}; it could not be stopped: {kill}")
                            }
                        });
                        continue;
                    }
                }
                match stado_wait::blocking(stado_wait::Kind::Process, "the clipboard writer", command, || {
                    child.wait_with_output()
                }) {
                    Ok(output) if output.status.success() => {
                        return verified(command, payload).map(|_reader| command.to_string())
                    }
                    Ok(output) => attempts.push(refused(command, &output)),
                    Err(error) => attempts.push(format!("{command}: {error}")),
                }
            }
            Err(error) => attempts.push(format!("{command} cannot be started: {error}")),
        }
    }
    Err(none_answered("writer", &attempts))
}

/// A writer that exits zero can still leave the clipboard untouched, and the
/// operator finds that out by pasting nothing: a hand-off reported as copied
/// while the clipboard holds something else entirely. So the claim is checked
/// before it is made.
fn verified(command: &str, payload: &str) -> Result<String, String> {
    let (found, reader) = match read_clipboard() {
        Ok(pair) => pair,
        Err(error) => {
            return Err(format!(
                "{command} accepted {} bytes but this host has no clipboard reader to confirm it landed: {error}",
                payload.len()
            ))
        }
    };
    if same_payload(&found, payload) {
        return Ok(reader);
    }
    let holds = match found.lines().next() {
        Some(first) => format!("{} bytes that are not the payload: it begins {:?}", found.len(), first.trim()),
        None => format!("{} bytes and no line, which is not the payload", found.len()),
    };
    Err(format!("{command} exited zero but {reader} reads {holds}. Nothing was handed over."))
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
        "Copied provided text to the OS clipboard with {command}, and read it back to confirm."
    ))
}
