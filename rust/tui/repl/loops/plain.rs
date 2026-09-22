//! What the prompt does when there is no terminal to draw on, and the small
//! pieces the interactive loop needs from the terminal itself.
//!
//! Split out of `tui/repl/loops.rs`, which had grown past the module line cap.

use std::io::{self, IsTerminal, Write};
use std::path::Path;
use std::sync::atomic::AtomicBool;
use std::sync::Arc;

use crossterm::event::{DisableBracketedPaste, EnableBracketedPaste};
use crossterm::terminal;

use crate::tui::{
    AttachmentSource, AttachmentTray, CommandOutcome, PromptStatus, TurnCtx, TurnKind,
};

pub(super) fn old_read_line_loop<S, C, H>(
    mut _status_provider: S,
    mut _classify: C,
    handler: H,
) -> io::Result<()>
where
    S: FnMut() -> PromptStatus,
    C: FnMut(&str) -> TurnKind,
    H: Fn(&str, &TurnCtx) -> Result<CommandOutcome, String>,
{
    let mut stdout = io::stdout();
    loop {
        let mut input = String::new();
        if io::stdin().read_line(&mut input)? == 0 {
            break;
        }
        let prompt = input.trim_end_matches(['\r', '\n']);
        if prompt.trim().is_empty() {
            continue;
        }
        if matches!(prompt.trim(), "/exit" | "/quit") {
            break;
        }
        let ctx = TurnCtx {
            cancel: Arc::new(AtomicBool::new(false)),
            interactive: false,
            from_view: false,
            attachments: &[],
            progress: &|_| {},
            stream: &|_| {},
            trace: &|_| {},
            ask_user: None,
            approve: &|_, _| false,
        };
        let (text, exit) = match handler(prompt, &ctx) {
            Ok(CommandOutcome::Exit(text)) => (text, true),
            Ok(outcome) => (outcome.into_text(), false),
            Err(error) => (format!("BŁĄD\t{error}"), false),
        };
        stdout.write_all(sanitize_terminal_text(&text).as_bytes())?;
        stdout.write_all(b"\n")?;
        if exit {
            break;
        }
    }
    stdout.flush()
}

pub(super) fn terminal_dimensions() -> (usize, usize) {
    terminal::size()
        .map(|(columns, rows)| (usize::from(columns).max(1), usize::from(rows).max(1)))
        .unwrap_or((100, 30))
}
pub(super) fn attachment_command(
    input: &str,
    cwd: &Path,
    tray: &mut AttachmentTray,
) -> Option<Result<String, String>> {
    let trimmed = input.trim();
    let (command, rest) = trimmed
        .split_once(char::is_whitespace)
        .unwrap_or((trimmed, ""));
    let rest = rest.trim();
    match command {
        "/attach" => Some(
            tray.add_file(cwd, rest)
                .map_err(|error| error.to_string())
                .map(|id| {
                    let item = tray
                        .items()
                        .iter()
                        .find(|item| item.id == id)
                        .expect("new attachment remains in tray");
                    format!("Attached #{} {}", id.0, item.fallback_label())
                }),
        ),
        "/attachments" => Some(if rest.is_empty() {
            if tray.items().is_empty() {
                Ok("No pending attachments.".into())
            } else {
                Ok(tray
                    .items()
                    .iter()
                    .map(|item| {
                        let provenance = match &item.source {
                            AttachmentSource::Clipboard => "clipboard".to_string(),
                            AttachmentSource::File { basename } => format!("file:{basename}"),
                        };
                        format!("#{} {} · {provenance}", item.id.0, item.fallback_label())
                    })
                    .collect::<Vec<_>>()
                    .join("\n"))
            }
        } else {
            Err("Usage: /attachments".into())
        }),
        "/detach" => Some(if rest == "all" {
            let count = tray.take_all().len();
            Ok(format!("Detached {count} attachment(s)."))
        } else {
            let id = if rest.is_empty() {
                tray.items()
                    .last()
                    .map(|item| item.id)
                    .ok_or_else(|| "No pending attachments to detach.".to_string())
            } else {
                rest.strip_prefix('#')
                    .unwrap_or(rest)
                    .parse::<u64>()
                    .map(super::super::AttachmentId)
                    .map_err(|_| "Usage: /detach [id|all]".to_string())
            };
            id.and_then(|id| {
                tray.remove(id)
                    .map(|item| format!("Detached #{} {}", id.0, item.fallback_label()))
                    .ok_or_else(|| format!("Attachment #{} is not in the tray.", id.0))
            })
        }),
        _ => None,
    }
}

pub(super) struct BracketedPasteGuard;

impl BracketedPasteGuard {
    pub(super) fn enter() -> io::Result<Self> {
        crossterm::execute!(io::stdout(), EnableBracketedPaste)?;
        Ok(Self)
    }
}

impl Drop for BracketedPasteGuard {
    fn drop(&mut self) {
        let _ = crossterm::execute!(io::stdout(), DisableBracketedPaste);
    }
}
