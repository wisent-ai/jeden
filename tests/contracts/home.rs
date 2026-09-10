//! The isolated home every contract journey runs in.
//!
//! One directory per journey under `target/contract-runs`, carrying its own
//! `HOME`, session root and workspace, the exact source revision, the digest
//! of the binary that ran, and one evidence file per command. Nothing here
//! reads or writes the operator's own sessions, configuration or workspace.

use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::cell::Cell;
use std::fs::{self, File};
use std::io::{BufRead, BufReader, Write};
use std::path::PathBuf;
use std::process::{Command, Output, Stdio};

pub(crate) struct Home {
    pub(crate) root: PathBuf,
    sequence: Cell<usize>,
}

impl Home {
    pub(crate) fn new(story: &str) -> Self {
        let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("target/contract-runs")
            .join(format!("{story}-{}", uuid::Uuid::new_v4()));
        for area in ["home", "workspace", "sessions", "evidence"] {
            fs::create_dir_all(root.join(area)).unwrap();
        }
        let revision = Command::new("git")
            .args(["rev-parse", "HEAD"])
            .current_dir(env!("CARGO_MANIFEST_DIR"))
            .output()
            .unwrap();
        assert!(revision.status.success());
        let mut binary = BufReader::new(File::open(env!("CARGO_BIN_EXE_jeden")).unwrap());
        let mut digest = Sha256::new();
        loop {
            let bytes = binary.fill_buf().unwrap();
            if bytes.is_empty() {
                break;
            }
            digest.update(bytes);
            let size = bytes.len();
            binary.consume(size);
        }
        fs::write(
            root.join("source.json"),
            serde_json::to_vec_pretty(&json!({
                "revision": String::from_utf8(revision.stdout).unwrap().trim(),
                "binary": env!("CARGO_BIN_EXE_jeden"), "sha256": format!("{:x}", digest.finalize()),
            }))
            .unwrap(),
        )
        .unwrap();
        println!("Retained evidence: {}", root.display());
        Self {
            root,
            sequence: Cell::new(usize::default()),
        }
    }
    pub(crate) fn workspace(&self) -> PathBuf {
        self.root.join("workspace")
    }
    fn command(&self) -> Command {
        let mut command = Command::new(env!("CARGO_BIN_EXE_jeden"));
        command
            .current_dir(self.workspace())
            .env("HOME", self.root.join("home"))
            .env("JEDEN_SESSION_ROOT", self.root.join("sessions"))
            .env_remove("JEDEN_LANGUAGE");
        command
    }
    fn record(&self, args: &[&str], output: &Output) {
        let number = self.sequence.get() + usize::from(true);
        self.sequence.set(number);
        fs::write(
            self.root.join("evidence").join(format!("{number}.json")),
            serde_json::to_vec_pretty(&json!({"argv": args, "exitCode": output.status.code(),
                "stdout": String::from_utf8_lossy(&output.stdout),
                "stderr": String::from_utf8_lossy(&output.stderr)}))
            .unwrap(),
        )
        .unwrap();
    }
    pub(crate) fn run(&self, args: &[&str]) -> Output {
        let output = self.command().args(args).output().unwrap();
        self.record(args, &output);
        output
    }
    pub(crate) fn ok(&self, args: &[&str]) -> Output {
        let output = self.run(args);
        assert!(
            output.status.success(),
            "{}: {}",
            args.join(" "),
            String::from_utf8_lossy(&output.stderr)
        );
        output
    }
    pub(crate) fn value(&self, args: &[&str]) -> Value {
        serde_json::from_slice(&self.ok(args).stdout).unwrap()
    }
    pub(crate) fn session(&self) -> PathBuf {
        let mode: Value = serde_json::from_slice(
            &fs::read(self.workspace().join(".jeden/mode-state.json")).unwrap(),
        )
        .unwrap();
        PathBuf::from(mode["lastSessionPath"].as_str().unwrap())
    }
    pub(crate) fn state(&self) -> Value {
        serde_json::from_slice(&fs::read(self.session().join("completion.json")).unwrap()).unwrap()
    }
    pub(crate) fn control(&self, id: &str, action: &str) -> Value {
        self.value(&[
            "todo",
            action,
            id,
            "--revision",
            &self.state()["revision"].to_string(),
            "--reason",
            "Isolated product journey",
            "--json",
        ])
    }
    pub(crate) fn rpc(&self, requests: &[Value]) -> Vec<Value> {
        fs::write(
            self.root
                .join("evidence")
                .join(format!("rpc-input-{}.json", self.sequence.get())),
            serde_json::to_vec_pretty(requests).unwrap(),
        )
        .unwrap();
        let mut child = self
            .command()
            .arg("rpc")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        let mut input = child.stdin.take().unwrap();
        let mut output_stream = BufReader::new(child.stdout.take().unwrap());
        let mut frames = Vec::new();
        let mut transcript = String::new();
        for request in requests {
            writeln!(input, "{request}").unwrap();
            input.flush().unwrap();
            loop {
                let mut line = String::new();
                assert!(
                    output_stream.read_line(&mut line).unwrap() > usize::default(),
                    "RPC closed before answering {request}"
                );
                let frame: Value = serde_json::from_str(&line).unwrap();
                let answered = frame["id"] == request["id"];
                transcript.push_str(&line);
                frames.push(frame);
                if answered {
                    break;
                }
            }
        }
        writeln!(
            input,
            "{}",
            json!({"id":"shutdown", "method":"shutdown", "params":{}})
        )
        .unwrap();
        drop(input);
        for line in output_stream.lines() {
            let line = line.unwrap();
            frames.push(serde_json::from_str(&line).unwrap());
            transcript.push_str(&line);
            transcript.push('\n');
        }
        let mut output = child.wait_with_output().unwrap();
        output.stdout = transcript.into_bytes();
        self.record(&["rpc"], &output);
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        frames
    }
    pub(crate) fn config(&self) -> Value {
        serde_json::from_slice(&fs::read(self.root.join("home/.jeden/config.yml")).unwrap())
            .unwrap()
    }
    pub(crate) fn passed(&self) {
        fs::write(
            self.root.join("result.json"),
            serde_json::to_vec(&json!({"passed":true})).unwrap(),
        )
        .unwrap();
    }
}
