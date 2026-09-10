//! Product contract journeys through the real CLI and RPC binary.
//! Model-dependent journeys use the configured Brama route and fail on its
//! actual refusal. Evidence and isolated state remain under target/contract-runs.
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::cell::Cell;
use std::fs::{self, File};
use std::io::{BufRead, BufReader, Write};
use std::path::PathBuf;
use std::process::{Command, Output, Stdio};

struct Home {
    root: PathBuf,
    sequence: Cell<usize>,
}
impl Home {
    fn new(story: &str) -> Self {
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
    fn workspace(&self) -> PathBuf {
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
    fn run(&self, args: &[&str]) -> Output {
        let output = self.command().args(args).output().unwrap();
        self.record(args, &output);
        output
    }
    fn ok(&self, args: &[&str]) -> Output {
        let output = self.run(args);
        assert!(
            output.status.success(),
            "{}: {}",
            args.join(" "),
            String::from_utf8_lossy(&output.stderr)
        );
        output
    }
    fn value(&self, args: &[&str]) -> Value {
        serde_json::from_slice(&self.ok(args).stdout).unwrap()
    }
    fn session(&self) -> PathBuf {
        let mode: Value = serde_json::from_slice(
            &fs::read(self.workspace().join(".jeden/mode-state.json")).unwrap(),
        )
        .unwrap();
        PathBuf::from(mode["lastSessionPath"].as_str().unwrap())
    }
    fn state(&self) -> Value {
        serde_json::from_slice(&fs::read(self.session().join("completion.json")).unwrap()).unwrap()
    }
    fn control(&self, id: &str, action: &str) -> Value {
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
    fn rpc(&self, requests: &[Value]) -> Vec<Value> {
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
    fn config(&self) -> Value {
        serde_json::from_slice(&fs::read(self.root.join("home/.jeden/config.yml")).unwrap())
            .unwrap()
    }
    fn passed(&self) {
        fs::write(
            self.root.join("result.json"),
            serde_json::to_vec(&json!({"passed":true})).unwrap(),
        )
        .unwrap();
    }
}

#[test]
fn operator_contracts_are_written_read_and_reset_through_the_cli() {
    let home = Home::new("settings-cli");
    home.ok(&[
        "config",
        "set",
        "contracts.communication",
        "Answer in Polish using three plain sentences.",
    ]);
    home.ok(&[
        "config",
        "set",
        "contracts.functionality",
        "Finish the requested behavior before answering.",
    ]);
    assert_eq!(
        home.config()["contracts"]["communication"],
        "Answer in Polish using three plain sentences."
    );
    assert_eq!(
        home.config()["contracts"]["functionality"],
        "Finish the requested behavior before answering."
    );
    let read = home.ok(&["config", "get", "contracts.functionality"]);
    assert_eq!(
        String::from_utf8(read.stdout).unwrap().trim(),
        home.config()["contracts"]["functionality"]
    );
    home.ok(&["config", "reset", "contracts.functionality"]);
    assert_eq!(home.config()["contracts"]["functionality"], "");
    let refused = home.run(&["config", "get", "contracts.style"]);
    assert!(!refused.status.success());
    assert_eq!(
        String::from_utf8(refused.stderr).unwrap().trim(),
        "Error: unknown config key: contracts.style"
    );
    home.passed();
}

#[test]
fn operator_contracts_are_served_through_rpc_for_jeden_desktop() {
    let home = Home::new("settings-rpc");
    let frames = home.rpc(&[
        json!({"id":"set", "method":"config/contracts/set", "params":{"communication":"Short sentences.", "functionality":"Finish the task in full."}}),
        json!({"id":"refuse", "method":"config/contracts/set", "params":{"communication":"Only one field."}}),
    ]);
    assert_eq!(
        home.config()["contracts"]["communication"],
        "Short sentences."
    );
    assert_eq!(
        home.config()["contracts"]["functionality"],
        "Finish the task in full."
    );
    let refused = frames.iter().find(|frame| frame["id"] == "refuse").unwrap();
    assert_eq!(refused["error"]["code"], "invalid_params");
    assert_eq!(
        refused["error"]["message"],
        "functionality must be a string"
    );
    assert_eq!(
        String::from_utf8(
            home.ok(&["config", "get", "contracts.functionality"])
                .stdout
        )
        .unwrap()
        .trim(),
        "Finish the task in full."
    );
    home.passed();
}

#[test]
fn contracts_install_preserves_surrounding_rules_and_replaces_stale_contract() {
    let home = Home::new("install");
    let path = home.root.join("consumer.txt");
    fs::write(&path, "Existing rule one.\n").unwrap();
    let file = path.to_str().unwrap();
    assert!(!home
        .run(&["contracts", "status", "--file", file])
        .status
        .success());
    home.ok(&["contracts", "install", "--file", file]);
    let first = fs::read_to_string(&path).unwrap();
    let rendered = String::from_utf8(home.ok(&["contracts", "render"]).stdout).unwrap();
    assert!(first.starts_with("Existing rule one.\n"));
    assert!(first.contains(rendered.trim()));
    home.ok(&["contracts", "install", "--file", file]);
    assert_eq!(fs::read_to_string(&path).unwrap(), first);
    home.ok(&[
        "config",
        "set",
        "contracts.communication",
        "Answer in three sentences.",
    ]);
    assert!(!home
        .run(&["contracts", "status", "--file", file])
        .status
        .success());
    home.ok(&["contracts", "install", "--file", file]);
    let current = fs::read_to_string(&path).unwrap();
    assert!(current.starts_with("Existing rule one.\n"));
    assert!(current.contains("Answer in three sentences."));
    assert!(!current.contains(rendered.trim()));
    home.ok(&["contracts", "status", "--file", file]);
    let refused = home.run(&["contracts", "install", "--file"]);
    assert!(!refused.status.success());
    assert_eq!(
        String::from_utf8(refused.stderr).unwrap().trim(),
        "Error: --file requires a path"
    );
    home.passed();
}

#[test]
fn retained_requests_survive_new_questions_pause_and_session_recovery() {
    let home = Home::new("retained-controls");
    let first = home.value(&[
        "todo",
        "add",
        "Create alpha.txt containing exactly ALPHA.",
        "--json",
    ]);
    let request = first["requests"][0]["id"].as_str().unwrap();
    let paused = home.control(request, "pause");
    assert_eq!(paused["complete"], false);
    let before = home.state();
    let stale = home.run(&[
        "todo",
        "resume",
        request,
        "--revision",
        &first["revision"].to_string(),
        "--reason",
        "Stale control",
    ]);
    assert!(!stale.status.success());
    assert_eq!(home.state(), before);
    let source = home.session();
    let resumed = home.run(&["resume", source.to_str().unwrap()]);
    assert!(!resumed.status.success());
    assert_eq!(
        String::from_utf8(resumed.stderr).unwrap().trim(),
        "Error: Retained work is paused; resume its tasks before continuing."
    );
    assert_ne!(home.session(), source);
    assert_eq!(home.state()["requests"], before["requests"]);
    let added = home.value(&[
        "todo",
        "add",
        "Explain the previous request without cancelling it.",
        "--json",
    ]);
    let second = added["requests"][1]["id"].as_str().unwrap();
    assert_eq!(home.state()["requests"][0], before["requests"][0]);
    let denied = home.run(&["todo", "done", request]);
    assert!(!denied.status.success());
    assert_eq!(home.state()["requests"][0], before["requests"][0]);
    home.control(request, "resume");
    assert_eq!(home.state()["requests"][0]["paused"], false);
    home.control(second, "cancel");
    assert_eq!(home.state()["requests"][0]["coverageVerified"], false);
    let cancelled = home.control(request, "cancel");
    assert_eq!(cancelled["status"], "cancelled");
    let final_state = home.state();
    let denied = home.run(&[
        "todo",
        "resume",
        request,
        "--revision",
        &final_state["revision"].to_string(),
        "--reason",
        "Cannot revive cancelled work",
    ]);
    assert!(!denied.status.success());
    assert_eq!(home.state(), final_state);
    home.passed();
}

#[test]
fn task_contract_delivery_observes_real_files_and_retains_results_on_resume() {
    let home = Home::new("real-completion");
    let prompt = "Create alpha.txt containing exactly ALPHA and beta.txt containing exactly BETA. Finish only after both exist with those contents. Do not run commands, open applications, contact anyone, or send notifications.";
    let output = home.run(&[
        "run",
        prompt,
        "--allow-write",
        "--max-steps",
        "24",
        "--json",
    ]);
    if !output.status.success() {
        assert!(home.state()["requests"]
            .as_array()
            .unwrap()
            .iter()
            .any(|request| request["prompt"] == prompt));
        assert!(
            !home.workspace().join("alpha.txt").exists()
                || home.state()["requests"][0]["coverageVerified"] == false
        );
        panic!(
            "Real model completion failed; not passed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }
    assert_eq!(
        fs::read(home.workspace().join("alpha.txt")).unwrap(),
        b"ALPHA"
    );
    assert_eq!(
        fs::read(home.workspace().join("beta.txt")).unwrap(),
        b"BETA"
    );
    let result: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(result["completion"]["complete"], true);
    for task in home.state()["tasks"].as_array().unwrap() {
        assert_eq!(task["status"], "done");
        for reference in task["verification"]["evidence"].as_array().unwrap() {
            let session = PathBuf::from(reference["sessionPath"].as_str().unwrap());
            let events = fs::read_to_string(session.join("transcript.jsonl")).unwrap();
            assert!(events
                .lines()
                .map(|line| serde_json::from_str::<Value>(line).unwrap())
                .any(|event| event["eventId"] == reference["eventId"]));
        }
    }
    let before = home.state()["requests"].clone();
    let modification = fs::metadata(home.workspace().join("alpha.txt"))
        .unwrap()
        .modified()
        .unwrap();
    home.ok(&["resume", home.session().to_str().unwrap()]);
    assert_eq!(home.state()["requests"], before);
    assert_eq!(
        fs::metadata(home.workspace().join("alpha.txt"))
            .unwrap()
            .modified()
            .unwrap(),
        modification
    );
    home.passed();
}

#[test]
fn graphical_clients_queue_requests_without_running_a_model() {
    let home = Home::new("queued-rpc");
    let options = json!({"cwd": home.workspace(), "allowWrite": true, "allowCommand": false});
    let frames = home.rpc(&[
        json!({"id":"new", "method":"session/new", "params":{"options":options}}),
        json!({"id":"add-mobile", "method":"session/completion/add", "params":{"sessionId":"session-1", "prompt":"Create queued.txt containing QUEUED."}}),
        json!({"id":"add-desktop", "method":"session/prompt", "params":{"sessionId":"session-1", "requestId":"desktop-add", "prompt":"/todo add \"Explain the queued work without cancelling it.\""}}),
        json!({"id":"empty", "method":"session/completion/add", "params":{"sessionId":"session-1", "prompt":""}}),
        json!({"id":"state", "method":"session/completion/get", "params":{"sessionId":"session-1"}}),
    ]);
    let source = frames.iter().find(|frame| frame["id"] == "new").unwrap()["result"]["sessionPath"]
        .as_str()
        .unwrap();
    let saved: Value =
        serde_json::from_slice(&fs::read(PathBuf::from(source).join("completion.json")).unwrap())
            .unwrap();
    let prompts = saved["requests"]
        .as_array()
        .unwrap()
        .iter()
        .map(|request| request["prompt"].as_str().unwrap())
        .collect::<Vec<_>>();
    assert_eq!(
        prompts,
        [
            "Create queued.txt containing QUEUED.",
            "Explain the queued work without cancelling it."
        ]
    );
    assert!(saved["requests"]
        .as_array()
        .unwrap()
        .iter()
        .all(|request| request["planned"] == false));
    assert!(!home.workspace().join("queued.txt").exists());
    let refused = frames.iter().find(|frame| frame["id"] == "empty").unwrap();
    assert_eq!(refused["error"]["code"], "invalid_params");
    let state = frames.iter().find(|frame| frame["id"] == "state").unwrap();
    assert_eq!(state["result"]["completion"]["complete"], false);
    home.passed();
}
