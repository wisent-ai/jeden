//! `jeden copy` — the clipboard hand-off, driven through the real binary and
//! the real operating-system clipboard.
//!
//! The case this verb was built for: a device-level gate refuses the agent, the
//! fix is known exactly, and the operator has to run it in their own shell. A
//! command printed into a transcript is not a hand-off — it has to be retyped,
//! and the retyping is where it gets lost. So the contract worth defending is
//! not "the writer was called": it is that what the operator pastes is byte for
//! byte what the product said it copied.
//!
//! The clipboard is operator data. Every case below reads what it finds, does
//! its work, and puts the original back, including when it panics.

use std::io::Write;
use std::process::{Command, Output, Stdio};
use std::sync::{LazyLock, Mutex, MutexGuard};

fn jeden() -> Command {
    Command::new(env!("CARGO_BIN_EXE_jeden"))
}

/// The clipboard is one resource for the whole machine, so cases that touch it
/// run one at a time rather than racing each other inside the test binary.
static CLIPBOARD: LazyLock<Mutex<()>> = LazyLock::new(|| Mutex::new(()));

fn clipboard_lock() -> MutexGuard<'static, ()> {
    CLIPBOARD
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// How this host reads and writes its clipboard, in the same order
/// `slash::session::clipboard` tries the writers. No entry at all is a real
/// state the product has to answer for, and `refuses_without_a_writer` is what
/// every case asserts on such a host.
struct Tools {
    read: (String, Vec<String>),
    write: (String, Vec<String>),
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

fn tools() -> Option<Tools> {
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
                read: ("xclip".to_string(), words(&["-selection", "clipboard", "-o"])),
                write: ("xclip".to_string(), words(&["-selection", "clipboard"])),
            },
        ],
    };
    candidates
        .into_iter()
        .find(|tool| runs(&tool.read.0, &tool.read.1))
}

fn read_clipboard(tools: &Tools) -> String {
    let output = Command::new(&tools.read.0)
        .args(&tools.read.1)
        .output()
        .expect("the clipboard reader runs");
    String::from_utf8_lossy(&output.stdout).to_string()
}

fn write_clipboard(tools: &Tools, payload: &str) {
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
struct Restored {
    tools: Tools,
    original: String,
}

impl Restored {
    fn new(tools: Tools) -> Self {
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
fn body(text: &str) -> &str {
    text.trim_end_matches('\n')
}

/// The multi-line block this verb exists to hand over: the consent line the
/// product itself prints, the checkout to stand in, and the commands that
/// follow. Its exact bytes are the thing under test.
fn handoff() -> String {
    [
        "export DEVICE_HOOK_EDIT_APPROVED=1",
        "cd ~/Documents/CodingProjects/Wisent/tama",
        "git apply .wisent-output/adaptive-candidate/repository-names-are-names.patch",
        "tests/browser_guard/verify.sh \"$HOME/.shared-hooks/block_browser_usage.sh\"",
    ]
    .join("\n")
}

fn copy_stdin(payload: &str, extra: &[&str]) -> Output {
    let mut child = jeden()
        .arg("copy")
        .arg("-")
        .args(extra)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("jeden copy - starts");
    child
        .stdin
        .as_mut()
        .expect("jeden copy - takes stdin")
        .write_all(payload.as_bytes())
        .expect("the payload reaches jeden");
    child.wait_with_output().expect("jeden copy - finishes")
}

/// A host with no clipboard command must be told so, rather than being told a
/// hand-off happened.
fn refuses_without_a_writer() {
    let output = jeden()
        .args(["copy", "anything"])
        .output()
        .expect("jeden copy runs");
    assert!(
        !output.status.success(),
        "a host with no clipboard writer must not report a successful hand-off"
    );
    assert!(
        !String::from_utf8_lossy(&output.stderr).trim().is_empty(),
        "the refusal has to name what went wrong"
    );
}

#[test]
fn text_given_on_the_command_line_is_what_the_operator_pastes() {
    let _serialised = clipboard_lock();
    let Some(tools) = tools() else {
        return refuses_without_a_writer();
    };
    let restored = Restored::new(tools);

    let payload = "tests/browser_guard/verify.sh ~/.shared-hooks/block_browser_usage.sh";
    let output = jeden()
        .args(["copy", payload])
        .output()
        .expect("jeden copy runs");

    assert!(output.status.success(), "jeden copy succeeded");
    let said = String::from_utf8_lossy(&output.stdout).to_string();
    assert!(
        said.contains(&format!("copied {} bytes", payload.len())),
        "the product reports the exact size it copied: {said}"
    );
    assert_eq!(body(&read_clipboard(&restored.tools)), payload);
}

#[test]
fn a_multi_line_block_survives_the_stdin_route_unchanged() {
    let _serialised = clipboard_lock();
    let Some(tools) = tools() else {
        return refuses_without_a_writer();
    };
    let restored = Restored::new(tools);

    let payload = handoff();
    let output = copy_stdin(&payload, &[]);

    assert!(output.status.success(), "jeden copy - succeeded");
    assert_eq!(
        body(&read_clipboard(&restored.tools)),
        payload,
        "every line of the hand-off has to arrive, in order and unedited"
    );
}

#[test]
fn the_json_form_names_the_writer_and_the_size() {
    let _serialised = clipboard_lock();
    let Some(tools) = tools() else {
        return refuses_without_a_writer();
    };
    let restored = Restored::new(tools);

    let payload = handoff();
    let output = copy_stdin(&payload, &["--json"]);
    let line = String::from_utf8_lossy(&output.stdout).to_string();

    assert!(output.status.success(), "jeden copy - --json succeeded");
    assert!(
        line.contains(&format!("\"copied\":{}", payload.len())),
        "the report carries the byte count: {line}"
    );
    assert!(
        line.contains(&format!("\"command\":\"{}\"", restored.tools.write.0)),
        "the report names the writer that took it: {line}"
    );
}

#[test]
fn nothing_to_copy_is_refused_instead_of_wiping_the_clipboard() {
    let _serialised = clipboard_lock();
    let Some(tools) = tools() else {
        return refuses_without_a_writer();
    };
    // The guard is taken before anything is written, so what the operator had
    // is what comes back — the case needs its own content on the clipboard to
    // prove a refusal left it alone.
    let restored = Restored::new(tools);
    let kept = "the operator's own clipboard content";
    write_clipboard(&restored.tools, kept);

    for arguments in [vec!["copy"], vec!["copy", "   "]] {
        let output = jeden().args(&arguments).output().expect("jeden copy runs");
        assert!(
            !output.status.success(),
            "{arguments:?} must be refused rather than silently emptying the clipboard"
        );
        assert_eq!(
            body(&read_clipboard(&restored.tools)),
            kept,
            "{arguments:?} left the clipboard changed"
        );
    }
}

/// The defect this pair defends: on 2026-09-11 a hand-off was reported as
/// copied, the operator pasted, and the clipboard held an unrelated 488 bytes.
/// Reporting a copy is a claim about the clipboard, so the product has to read
/// it back, and a caller has to be able to ask again later.
#[test]
fn a_copy_is_confirmed_by_reading_the_clipboard_back() {
    let _serialised = clipboard_lock();
    let Some(tools) = tools() else {
        return refuses_without_a_writer();
    };
    let restored = Restored::new(tools);

    let payload = handoff();
    let output = copy_stdin(&payload, &[]);
    let said = String::from_utf8_lossy(&output.stdout).to_string();

    assert!(output.status.success(), "jeden copy - succeeded: {said}");
    assert!(
        said.contains("read them back to confirm"),
        "a copy reports the confirmation, not just the call: {said}"
    );
    assert_eq!(body(&read_clipboard(&restored.tools)), payload);

    let checked = copy_stdin(&payload, &["--check"]);
    assert!(
        checked.status.success(),
        "the same payload still on the clipboard is a passing check: {}",
        String::from_utf8_lossy(&checked.stderr)
    );
    assert!(
        String::from_utf8_lossy(&checked.stdout).contains("still holds"),
        "the check says what it found"
    );
}

#[test]
fn a_clipboard_replaced_after_the_copy_fails_the_check() {
    let _serialised = clipboard_lock();
    let Some(tools) = tools() else {
        return refuses_without_a_writer();
    };
    let restored = Restored::new(tools);

    let payload = handoff();
    assert!(copy_stdin(&payload, &[]).status.success(), "the copy lands");
    // What happened on the operator's machine: something else copied after the
    // hand-off, and nothing said so.
    write_clipboard(&restored.tools, "Hey Lukasz, Julien,");

    let checked = copy_stdin(&payload, &["--check"]);
    let said = String::from_utf8_lossy(&checked.stderr).to_string();

    assert!(
        !checked.status.success(),
        "a replaced clipboard is not a hand-off"
    );
    assert!(
        said.contains("no longer holds the payload") && said.contains("Copy it again"),
        "the refusal names what is there instead and what to do: {said}"
    );
    assert_eq!(
        body(&read_clipboard(&restored.tools)),
        "Hey Lukasz, Julien,",
        "a check writes nothing"
    );
}
