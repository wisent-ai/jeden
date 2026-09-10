//! `jeden copy` — hand an exact payload to the operator's clipboard.
//!
//! `/copy <text>` has always been able to do this inside an interactive
//! session, and nothing else could: the writer was reachable only from the
//! slash layer, so an agent, a script, or a CI job that had produced the exact
//! command an operator has to run in their own shell could only print it and
//! hope it was read and retyped. That is the case this verb exists for — a
//! device-level gate refuses the agent, the fix is known, and the commands
//! belong on the operator's clipboard rather than in prose.
//!
//! Both surfaces share `slash::session::clipboard::write_clipboard`, so the
//! platform order (`pbcopy`, then the Windows and Wayland/X11 writers) is
//! decided once and reported the same way here and there.

use std::io::{IsTerminal, Read};

use crate::slash::write_clipboard;
use crate::Args;

/// The payload the operator asked for: the argument list, or stdin when the
/// single positional is `-`.
fn payload(args: &Args) -> Result<String, String> {
    let positionals: Vec<&str> = args.positionals.iter().map(String::as_str).collect();
    match positionals.as_slice() {
        [] => Err(
            "copy requires the text to hand over, or - to read the payload from stdin".to_string(),
        ),
        ["-"] => {
            if std::io::stdin().is_terminal() {
                return Err(
                    "copy - reads the payload from stdin, and stdin is a terminal here".to_string(),
                );
            }
            let mut text = String::new();
            std::io::stdin()
                .read_to_string(&mut text)
                .map_err(|error| format!("cannot read the payload from stdin: {error}"))?;
            Ok(text)
        }
        words => Ok(words.join(" ")),
    }
}

/// `jeden copy <text> | jeden copy - [--json]`
pub(crate) fn copy_command(args: &Args) -> Result<String, String> {
    let payload = payload(args)?;
    if payload.trim().is_empty() {
        return Err(
            "refusing to copy an empty payload: it would replace what is on the clipboard with nothing"
                .to_string(),
        );
    }
    let command = write_clipboard(&payload)?;
    let bytes = payload.len();
    if args.json {
        return serde_json::to_string(&serde_json::json!({
            "copied": bytes,
            "command": command,
        }))
        .map(|line| line + "\n")
        .map_err(|error| error.to_string());
    }
    Ok(format!(
        "copied {bytes} bytes to the clipboard with {command}\n"
    ))
}
