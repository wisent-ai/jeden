//! Reading and writing the host clipboard for the `jeden copy` cases, and
//! putting the operator's own content back afterwards.

use std::io::Write;
use std::process::{Command, Stdio};

/// How this host reads and writes its clipboard, in the same order
/// `slash::session::clipboard` tries the writers. No entry at all is a real
/// state the product has to answer for, and `refuses_without_a_writer` is what
/// every case asserts on such a host.
pub struct Tools {
    pub read: (String, Vec<String>),
    pub write: (String, Vec<String>),
}

fn words(items: &[&str]) -> Vec<String> {
    items.iter().map(|item| item.to_string()).collect()
}

fn runs(command: &str, args: &[String]) -> bool {
    Command::new(command)
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .is_ok()
}

pub fn tools() -> Option<Tools> {
    let candidates: Vec<Tools> = match std::env::consts::OS {
        "macos" => vec![Tools {
            read: ("pbpaste".to_string(), Vec::new()),
            write: ("pbcopy".to_string(), Vec::new()),
        }],
        "windows" => vec![Tools {
            read: (
                "powershell.exe".to_string(),
                words(&["-NoProfile", "-NonInteractive", "-Command", "Get-Clipboard"]),
            ),
            write: (
                "powershell.exe".to_string(),
                words(&[
                    "-NoProfile",
                    "-NonInteractive",
                    "-Command",
                    "Set-Clipboard -Value ([Console]::In.ReadToEnd())",
                ]),
            ),
        }],
        _ => vec![
            Tools {
                read: ("wl-paste".to_string(), words(&["--no-newline"])),
                write: ("wl-copy".to_string(), Vec::new()),
            },
            Tools {
                read: (
                    "xclip".to_string(),
                    words(&["-selection", "clipboard", "-o"]),
                ),
                write: ("xclip".to_string(), words(&["-selection", "clipboard"])),
            },
        ],
    };
    candidates
        .into_iter()
        .find(|tool| runs(&tool.read.0, &tool.read.1))
}

pub fn read_clipboard(tools: &Tools) -> String {
    let output = Command::new(&tools.read.0)
        .args(&tools.read.1)
        .output()
        .expect("the clipboard reader runs");
    String::from_utf8_lossy(&output.stdout).to_string()
}

pub fn write_clipboard(tools: &Tools, payload: &str) {
    let mut child = Command::new(&tools.write.0)
        .args(&tools.write.1)
        .stdin(Stdio::piped())
        .spawn()
        .expect("the clipboard writer runs");
    child
        .stdin
        .as_mut()
        .expect("the writer takes stdin")
        .write_all(payload.as_bytes())
        .expect("the payload reaches the writer");
    child.wait().expect("the writer finishes");
}

/// Holds what the operator had, and puts it back even when a case fails.
pub struct Restored {
    pub tools: Tools,
    original: String,
}

impl Restored {
    pub fn new(tools: Tools) -> Self {
        let original = read_clipboard(&tools);
        Self { tools, original }
    }
}

impl Drop for Restored {
    fn drop(&mut self) {
        write_clipboard(&self.tools, &self.original);
    }
}

/// Whether a writer keeps a trailing newline is the writer's business; the
/// payload's own text is what has to survive the round trip.
pub fn body(text: &str) -> &str {
    text.trim_end_matches('\n')
}
