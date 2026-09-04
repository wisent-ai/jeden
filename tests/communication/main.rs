//! Communication modes through the real `jeden` binary: `jeden config`
//! writes and reads the user configuration on disk, `jeden rpc` reads and
//! writes the same values for Jeden Desktop, and both refuse values outside
//! the schema with the sentences below, copied from live answers.
//!
//! Isolation: every test gets its own `HOME` (so `~/.jeden/config.yml` is
//! the test's file) and its own `JEDEN_SESSION_ROOT`. Nothing here touches the
//! operator's configuration or sessions.
//!
//! Run: `cargo test --test communication -- --nocapture`

use serde_json::{json, Value};
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

fn jeden() -> Command {
    Command::new(env!("CARGO_BIN_EXE_jeden"))
}

struct Home {
    root: PathBuf,
}

impl Home {
    fn new(tag: &str) -> Self {
        let root =
            std::env::temp_dir().join(format!("jeden-communication-{tag}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(root.join("home")).expect("create isolated home");
        fs::create_dir_all(root.join("sessions")).expect("create isolated session root");
        Self { root }
    }

    fn home(&self) -> PathBuf {
        self.root.join("home")
    }

    fn config_file(&self) -> PathBuf {
        self.home().join(".jeden/config.yml")
    }

    fn command(&self) -> Command {
        let mut command = jeden();
        command
            .env("HOME", self.home())
            .env("JEDEN_SESSION_ROOT", self.root.join("sessions"))
            .env_remove("JEDEN_LANGUAGE")
            .current_dir(&self.root);
        command
    }

    fn config(&self, args: &[&str]) -> (i32, String, String) {
        let output = self
            .command()
            .arg("config")
            .args(args)
            .output()
            .expect("run jeden config");
        (
            output.status.code().unwrap_or(-1),
            String::from_utf8_lossy(&output.stdout).into_owned(),
            String::from_utf8_lossy(&output.stderr).into_owned(),
        )
    }

    /// Drive `jeden rpc` with one request after the `ready` frame and return
    /// the response frame with that id.
    fn rpc(&self, id: &str, method: &str, params: Value) -> Value {
        let mut child = self
            .command()
            .arg("rpc")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .expect("spawn jeden rpc");
        let mut stdin = child.stdin.take().expect("rpc stdin");
        let request = json!({"id": id, "method": method, "params": params});
        let shutdown = json!({"id": "shutdown", "method": "shutdown", "params": {}});
        write!(stdin, "{request}\n{shutdown}\n").expect("write rpc frames");
        drop(stdin);
        let output = child.wait_with_output().expect("wait for jeden rpc");
        let frames = String::from_utf8_lossy(&output.stdout);
        frames
            .lines()
            .filter_map(|line| serde_json::from_str::<Value>(line).ok())
            .find(|frame| frame.get("id").and_then(Value::as_str) == Some(id))
            .unwrap_or_else(|| panic!("no rpc frame with id {id} in:\n{frames}"))
    }
}

fn file_value(path: &Path, pointer: &str) -> Value {
    let text = fs::read_to_string(path).expect("read user config");
    serde_json::from_str::<Value>(&text)
        .expect("user config is JSON")
        .pointer(pointer)
        .cloned()
        .unwrap_or(Value::Null)
}

#[test]
fn config_set_writes_the_mode_and_get_reads_it_back() {
    let home = Home::new("cli");

    // Defaults before any write: no file, schema defaults from `get`.
    assert!(!home.config_file().exists());
    let (code, stdout, _) = home.config(&["get", "communication.mode"]);
    assert_eq!(code, 0);
    assert_eq!(stdout, "normal\n");

    // A mode and one override land in the user file.
    let (code, stdout, _) = home.config(&["set", "communication.mode", "debug"]);
    assert_eq!(code, 0, "stdout: {stdout}");
    assert!(
        stdout.starts_with("Set communication.mode in "),
        "unexpected set output: {stdout}"
    );
    let (code, _, _) = home.config(&["set", "communication.toolResults", "hide"]);
    assert_eq!(code, 0);
    assert_eq!(
        file_value(&home.config_file(), "/communication/mode"),
        json!("debug")
    );
    assert_eq!(
        file_value(&home.config_file(), "/communication/toolResults"),
        json!("hide")
    );
    assert_eq!(file_value(&home.config_file(), "/schemaVersion"), json!(4));

    // `get --json` carries the schema the pickers and Desktop rely on.
    let (code, stdout, _) = home.config(&["get", "communication.code", "--json"]);
    assert_eq!(code, 0);
    let metadata: Value = serde_json::from_str(&stdout).expect("json metadata");
    assert_eq!(metadata["value"], json!("auto"));
    assert_eq!(metadata["type"], json!("enum"));
    assert_eq!(metadata["default"], json!("auto"));
    assert_eq!(metadata["enum"], json!(["auto", "show", "hide"]));

    // Reset returns the schema default and says so.
    let (code, stdout, _) = home.config(&["reset", "communication.toolResults"]);
    assert_eq!(code, 0);
    assert!(stdout.starts_with("Reset communication.toolResults to schema default in "));
    assert_eq!(
        file_value(&home.config_file(), "/communication/toolResults"),
        json!("auto")
    );
}

#[test]
fn config_set_refuses_values_outside_the_schema() {
    let home = Home::new("refusals");

    let (code, _, stderr) = home.config(&["set", "communication.mode", "loud"]);
    assert_eq!(code, 1);
    assert_eq!(
        stderr.trim(),
        "Error: communication.mode must be one of: normal, debug, quiet"
    );

    let (code, _, stderr) = home.config(&["set", "communication.reasoning", "maybe"]);
    assert_eq!(code, 1);
    assert_eq!(
        stderr.trim(),
        "Error: communication.reasoning must be one of: auto, show, hide"
    );

    let (code, _, stderr) = home.config(&["get", "communication.verbosity"]);
    assert_eq!(code, 1);
    assert_eq!(
        stderr.trim(),
        "Error: unknown config key: communication.verbosity"
    );

    // A refused write leaves no file behind.
    assert!(!home.config_file().exists());
}

#[test]
fn rpc_reads_and_writes_the_same_settings_for_jeden_desktop() {
    let home = Home::new("rpc");

    let result = home.rpc("communication-get", "config/communication/get", json!({}));
    let settings = &result["result"];
    assert_eq!(settings["mode"], json!("normal"));
    assert_eq!(settings["toolCalls"], json!("auto"));
    assert_eq!(settings["code"], json!("auto"));
    assert_eq!(
        settings["effective"],
        json!({
            "mode": "normal",
            "toolCalls": true,
            "toolCallDetail": false,
            "toolResults": false,
            "reasoning": false,
            "code": true
        })
    );
    assert_eq!(
        settings["path"],
        json!(home.config_file().display().to_string())
    );

    // A save resolves the overrides and writes every one of them.
    let result = home.rpc(
        "communication-set",
        "config/communication/set",
        json!({
            "mode": "debug",
            "toolCalls": "auto",
            "toolResults": "hide",
            "reasoning": "auto",
            "code": "hide"
        }),
    );
    let settings = &result["result"];
    assert_eq!(settings["mode"], json!("debug"));
    assert_eq!(settings["toolResults"], json!("hide"));
    assert_eq!(
        settings["effective"],
        json!({
            "mode": "debug",
            "toolCalls": true,
            "toolCallDetail": true,
            "toolResults": false,
            "reasoning": true,
            "code": false
        })
    );
    assert_eq!(
        file_value(&home.config_file(), "/communication/mode"),
        json!("debug")
    );
    assert_eq!(
        file_value(&home.config_file(), "/communication/code"),
        json!("hide")
    );

    // The CLI reads what the RPC wrote: one file, two surfaces.
    let (code, stdout, _) = home.config(&["get", "communication.mode"]);
    assert_eq!(code, 0);
    assert_eq!(stdout, "debug\n");

    // A value outside the schema is refused before anything is written.
    let result = home.rpc(
        "communication-bad",
        "config/communication/set",
        json!({
            "mode": "loud",
            "toolCalls": "auto",
            "toolResults": "auto",
            "reasoning": "auto",
            "code": "auto"
        }),
    );
    assert_eq!(result["error"]["code"], json!("config_write_failed"));
    assert_eq!(
        result["error"]["message"],
        json!("communication.mode must be one of: normal, debug, quiet")
    );
    assert_eq!(
        file_value(&home.config_file(), "/communication/mode"),
        json!("debug")
    );

    // A missing field is an invalid request, not a partial write.
    let result = home.rpc(
        "communication-short",
        "config/communication/set",
        json!({"mode": "quiet"}),
    );
    assert_eq!(result["error"]["code"], json!("invalid_params"));
    assert_eq!(
        result["error"]["message"],
        json!("toolCalls must be a non-empty string")
    );
    assert_eq!(
        file_value(&home.config_file(), "/communication/mode"),
        json!("debug")
    );
}
