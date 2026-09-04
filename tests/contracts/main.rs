//! Contracts through the real `jeden` binary.
//!
//! `operator_contracts_*` drive `jeden config` and `jeden rpc` against an
//! isolated `HOME` and read the file they wrote. `task_contract_*` drive real
//! `jeden run` turns through the configured Brama route and read the session
//! ledger those turns produced: the delivery-report rule is judged on the
//! answer Jeden delivered and on the `task_contract`, `task_report`, and
//! `contract_violation` events it recorded.
//!
//! A model turn needs the same environment the binary needs: `BRAMA_URL`,
//! `BRAMA_TOKEN`, `WISENT_APP_AGENT_AUTH_SECRET`, `WISENT_APP_AGENT_ID`, and
//! `JEDEN_MODEL`. `scripts/run-with-stado.sh` exports them on a configured
//! workstation; a missing one fails the test by name instead of skipping it.
//!
//! Run: `cargo test --test contracts -- --nocapture`

use serde_json::{json, Value};
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

const MODEL_TURN_ENV: [&str; 5] = [
    "BRAMA_URL",
    "BRAMA_TOKEN",
    "WISENT_APP_AGENT_AUTH_SECRET",
    "WISENT_APP_AGENT_ID",
    "JEDEN_MODEL",
];

const REQUIREMENTS: [&str; 7] = [
    "functionality",
    "diagnostics",
    "cli",
    "gui",
    "documentation",
    "tests",
    "delivery",
];

fn jeden() -> Command {
    Command::new(env!("CARGO_BIN_EXE_jeden"))
}

struct Home {
    root: PathBuf,
}

impl Home {
    fn new(tag: &str) -> Self {
        let root =
            std::env::temp_dir().join(format!("jeden-contracts-{tag}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(root.join("home")).expect("create isolated home");
        fs::create_dir_all(root.join("sessions")).expect("create isolated session root");
        fs::create_dir_all(root.join("workspace")).expect("create workspace");
        Self { root }
    }

    fn home(&self) -> PathBuf {
        self.root.join("home")
    }

    fn sessions(&self) -> PathBuf {
        self.root.join("sessions")
    }

    fn workspace(&self) -> PathBuf {
        self.root.join("workspace")
    }

    fn config_file(&self) -> PathBuf {
        self.home().join(".jeden/config.yml")
    }

    fn command(&self) -> Command {
        let mut command = jeden();
        command
            .env("HOME", self.home())
            .env("JEDEN_SESSION_ROOT", self.sessions())
            .env_remove("JEDEN_LANGUAGE")
            .current_dir(self.workspace());
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

    /// One real `jeden run --json` turn through Brama. A delivered turn prints
    /// one JSON object; a refused turn exits 1 with `Error: …` on stderr and
    /// prints nothing. Either way the session ledger the turn wrote is read.
    fn run_turn(&self, task: &str, extra: &[&str]) -> (Outcome, Vec<Value>) {
        for name in MODEL_TURN_ENV {
            assert!(
                std::env::var_os(name).is_some_and(|value| !value.is_empty()),
                "{name} is not set; the model turn needs the same environment the jeden binary needs"
            );
        }
        let before = self.session_dirs();
        let output = self
            .command()
            .args(["run", task, "--max-steps", "8", "--json"])
            .args(extra)
            .output()
            .expect("run jeden run");
        let stdout = String::from_utf8_lossy(&output.stdout);
        let stderr = String::from_utf8_lossy(&output.stderr);
        let session = self
            .session_dirs()
            .into_iter()
            .find(|dir| !before.contains(dir))
            .unwrap_or_else(|| panic!("the turn wrote no session; stderr:\n{stderr}"));
        let outcome = if output.status.success() {
            let report: Value = serde_json::from_str(stdout.trim()).unwrap_or_else(|error| {
                panic!(
                    "jeden run --json did not print one JSON object ({error}); stdout:\n{stdout}"
                )
            });
            assert_eq!(report["ok"], json!(true), "{report}");
            assert_eq!(
                report["sessionPath"].as_str().map(PathBuf::from),
                Some(session.clone()),
                "the printed sessionPath is the session the turn wrote"
            );
            Outcome::Delivered(report["text"].as_str().unwrap_or_default().to_string())
        } else {
            assert_eq!(output.status.code(), Some(1), "stderr:\n{stderr}");
            assert!(
                stdout.trim().is_empty(),
                "a refused turn prints no answer:\n{stdout}"
            );
            Outcome::Refused(stderr.trim().to_string())
        };
        (outcome, transcript_events(&session))
    }

    fn session_dirs(&self) -> Vec<PathBuf> {
        fs::read_dir(self.sessions())
            .map(|entries| {
                entries
                    .filter_map(Result::ok)
                    .map(|entry| entry.path())
                    .filter(|path| path.is_dir())
                    .collect()
            })
            .unwrap_or_default()
    }
}

enum Outcome {
    Delivered(String),
    Refused(String),
}

fn file_value(path: &Path, pointer: &str) -> Value {
    let text = fs::read_to_string(path).expect("read user config");
    serde_json::from_str::<Value>(&text)
        .expect("user config is JSON")
        .pointer(pointer)
        .cloned()
        .unwrap_or(Value::Null)
}

/// Every event of a session ledger as `{kind, data}`, in append order.
fn transcript_events(session: &Path) -> Vec<Value> {
    let text = fs::read_to_string(session.join("transcript.jsonl")).expect("read transcript");
    text.lines()
        .filter_map(|line| serde_json::from_str::<Value>(line).ok())
        .map(|record| {
            let (kind, data) = match record.get("payload") {
                Some(payload) => (payload["type"].clone(), payload["data"].clone()),
                None => (record["type"].clone(), record["data"].clone()),
            };
            json!({"kind": kind, "data": data})
        })
        .collect()
}

fn events_of<'a>(events: &'a [Value], kind: &str) -> Vec<&'a Value> {
    events
        .iter()
        .filter(|event| event["kind"].as_str() == Some(kind))
        .collect()
}

#[test]
fn operator_contracts_are_written_read_and_reset_through_the_cli() {
    let home = Home::new("cli");

    let (code, stdout, _) = home.config(&["get", "contracts.communication"]);
    assert_eq!(code, 0);
    assert_eq!(stdout, "\n", "an unset contract reads as empty");

    let (code, stdout, _) = home.config(&[
        "set",
        "contracts.communication",
        "Answer in Polish using three plain sentences.",
    ]);
    assert_eq!(code, 0, "stdout: {stdout}");
    assert!(stdout.starts_with("Set contracts.communication in "));
    let (code, _, _) = home.config(&[
        "set",
        "contracts.functionality",
        "Finish the requested behavior before answering.",
    ]);
    assert_eq!(code, 0);
    assert_eq!(
        file_value(&home.config_file(), "/contracts/communication"),
        json!("Answer in Polish using three plain sentences.")
    );
    assert_eq!(
        file_value(&home.config_file(), "/contracts/functionality"),
        json!("Finish the requested behavior before answering.")
    );

    let (code, stdout, _) = home.config(&["get", "contracts.functionality"]);
    assert_eq!(code, 0);
    assert_eq!(stdout, "Finish the requested behavior before answering.\n");

    let (code, _, _) = home.config(&["reset", "contracts.functionality"]);
    assert_eq!(code, 0);
    assert_eq!(
        file_value(&home.config_file(), "/contracts/functionality"),
        json!("")
    );

    let (code, _, stderr) = home.config(&["get", "contracts.style"]);
    assert_eq!(code, 1);
    assert_eq!(stderr.trim(), "Error: unknown config key: contracts.style");
}

#[test]
fn operator_contracts_and_task_contract_are_served_through_rpc_for_jeden_desktop() {
    let home = Home::new("rpc");

    let result = home.rpc("contracts-get", "config/contracts/get", json!({}));
    assert_eq!(result["result"]["communication"], json!(""));
    assert_eq!(result["result"]["functionality"], json!(""));
    assert_eq!(
        result["result"]["path"],
        json!(home.config_file().display().to_string())
    );
    // Nothing configured: Jeden's own communication contract is in force and
    // the Settings screen gets its text to show.
    assert_eq!(result["result"]["communicationSource"], json!("default"));
    assert!(result["result"]["communicationDefault"]
        .as_str()
        .is_some_and(|text| text.starts_with("Write to the user in plain language:")));
    // The built-in task contract rides along, read-only, for the Settings screen.
    let contract = &result["result"]["taskContract"];
    assert_eq!(contract["version"], json!(1));
    assert!(contract["instructions"]
        .as_str()
        .is_some_and(|text| text.starts_with("Task contract:")));
    let ids = contract["requirements"]
        .as_array()
        .expect("requirements array")
        .iter()
        .map(|requirement| requirement["id"].as_str().unwrap_or_default())
        .collect::<Vec<_>>();
    assert_eq!(ids, REQUIREMENTS);

    let result = home.rpc(
        "contracts-set",
        "config/contracts/set",
        json!({
            "communication": "Short sentences.",
            "functionality": "Finish the task in full."
        }),
    );
    assert_eq!(result["result"]["communication"], json!("Short sentences."));
    assert_eq!(
        result["result"]["functionality"],
        json!("Finish the task in full.")
    );
    assert_eq!(result["result"]["communicationSource"], json!("operator"));
    assert_eq!(
        file_value(&home.config_file(), "/contracts/communication"),
        json!("Short sentences.")
    );

    // The CLI reads what the RPC wrote.
    let (code, stdout, _) = home.config(&["get", "contracts.functionality"]);
    assert_eq!(code, 0);
    assert_eq!(stdout, "Finish the task in full.\n");

    // A missing field is refused as invalid parameters; nothing is written.
    let result = home.rpc(
        "contracts-short",
        "config/contracts/set",
        json!({"communication": "Only one field."}),
    );
    assert_eq!(result["error"]["code"], json!("invalid_params"));
    assert_eq!(
        result["error"]["message"],
        json!("functionality must be a string")
    );
    assert_eq!(
        file_value(&home.config_file(), "/contracts/communication"),
        json!("Short sentences.")
    );

    // `none` turns the default off; Jeden says so.
    let result = home.rpc(
        "contracts-none",
        "config/contracts/set",
        json!({"communication": "none", "functionality": ""}),
    );
    assert_eq!(result["result"]["communicationSource"], json!("disabled"));
}

#[test]
fn task_contract_is_in_every_system_prompt() {
    let home = Home::new("prompt");
    let output = home
        .command()
        .args(["run", "/prompt"])
        .output()
        .expect("run jeden run /prompt");
    let prompt = String::from_utf8_lossy(&output.stdout);
    assert!(
        prompt.contains("Task contract:"),
        "the system prompt has no task contract:\n{prompt}"
    );
    for id in REQUIREMENTS {
        assert!(
            prompt.contains(&format!("- {id} (")),
            "requirement {id} is missing from the prompt:\n{prompt}"
        );
    }
    assert!(prompt.contains("creates, edits and deletes a real isolated fleet through the CLI"));
    // With nothing configured, Jeden's default communication contract is in
    // the prompt: plain language, then what was done, blockers, next steps.
    assert!(
        prompt.contains(
            "Communication contract (Jeden default):\nWrite to the user in plain language:"
        ),
        "{prompt}"
    );
    assert!(prompt.contains("\"What was done\""));
    assert!(prompt.contains("\"Blockers\""));
    assert!(prompt.contains("\"Next steps\""));

    // The operator's own text replaces the default; `none` removes it.
    let (code, _, _) = home.config(&[
        "set",
        "contracts.communication",
        "Answer in three sentences.",
    ]);
    assert_eq!(code, 0);
    let prompt = String::from_utf8_lossy(
        &home
            .command()
            .args(["run", "/prompt"])
            .output()
            .expect("run jeden run /prompt with an operator contract")
            .stdout,
    )
    .into_owned();
    assert!(
        prompt.contains("Communication contract:\nAnswer in three sentences."),
        "{prompt}"
    );
    assert!(!prompt.contains("Jeden default"), "{prompt}");
    let (code, _, _) = home.config(&["set", "contracts.communication", "none"]);
    assert_eq!(code, 0);
    let prompt = String::from_utf8_lossy(
        &home
            .command()
            .args(["run", "/prompt"])
            .output()
            .expect("run jeden run /prompt with the contract turned off")
            .stdout,
    )
    .into_owned();
    assert!(!prompt.contains("Communication contract"), "{prompt}");

    // Polish conversations get the Polish contracts.
    let (code, _, _) = home.config(&["reset", "contracts.communication"]);
    assert_eq!(code, 0);
    let (code, _, _) = home.config(&["set", "ui.language", "pl"]);
    assert_eq!(code, 0);
    let output = home
        .command()
        .args(["run", "/prompt"])
        .output()
        .expect("run jeden run /prompt in Polish");
    let prompt = String::from_utf8_lossy(&output.stdout);
    assert!(prompt.contains("Kontrakt zadania:"), "{prompt}");
    assert!(prompt.contains("- tests (Realne testy):"), "{prompt}");
    assert!(
        prompt.contains(
            "Communication contract (Jeden default):\nPisz do użytkownika prostym językiem:"
        ),
        "{prompt}"
    );
}

#[test]
fn task_contract_delivery_report_is_enforced_on_a_real_turn() {
    let home = Home::new("turn");
    fs::write(
        home.workspace().join("NOTES.md"),
        "The deploy target is charless-mac-mini.\n",
    )
    .expect("seed workspace file");

    // An ordinary turn: the contract is recorded, the report is required, and
    // Jeden either delivers a rendered report or refuses the turn.
    let (outcome, events) = home.run_turn(
        "Use the read_file tool to read NOTES.md and tell me the deploy target in one sentence.",
        &[],
    );
    let contracts = events_of(&events, "task_contract");
    assert_eq!(
        contracts.len(),
        1,
        "one task_contract per turn; events: {events:?}"
    );
    assert_eq!(contracts[0]["data"]["version"], json!(1));
    assert!(contracts[0]["data"]["task"]
        .as_str()
        .is_some_and(|task| task.starts_with("Use the read_file tool")));
    assert!(
        !events_of(&events, "tool_call").is_empty(),
        "the turn ran no tool; events: {events:?}"
    );

    let violations = events_of(&events, "contract_violation");
    for violation in &violations {
        assert_eq!(violation["data"]["rule"], json!("delivery-report"));
    }
    let requested = violations
        .iter()
        .filter(|violation| violation["data"]["outcome"] == json!("requested"))
        .count();
    let rejected = violations
        .iter()
        .filter(|violation| violation["data"]["outcome"] == json!("rejected"))
        .count();
    assert!(
        requested <= 1,
        "the report is requested at most once; got {requested}"
    );

    match outcome {
        Outcome::Delivered(answer) => {
            // Delivered: the answer ends with the rendered report and the ledger
            // holds the structured one with every requirement explained.
            let answer = answer.as_str();
            assert!(
                answer.contains("How it was done"),
                "a delivered answer carries the rendered report:\n{answer}"
            );
            let reports = events_of(&events, "task_report");
            assert_eq!(reports.len(), 1, "one task_report per delivered turn");
            let report = &reports[0]["data"];
            assert!(matches!(
                report["status"].as_str(),
                Some("complete") | Some("blocked")
            ));
            for id in REQUIREMENTS {
                let entry = &report["report"][id];
                assert!(
                    matches!(
                        entry["status"].as_str(),
                        Some("done") | Some("not_applicable") | Some("blocked")
                    ),
                    "{id} has no status: {entry}"
                );
                assert!(
                    entry["explanation"]
                        .as_str()
                        .is_some_and(|text| !text.trim().is_empty()),
                    "{id} has no explanation: {entry}"
                );
                if entry["status"] == json!("done") {
                    assert!(
                        entry["evidence"]
                            .as_array()
                            .is_some_and(|list| !list.is_empty()),
                        "{id} is done without evidence: {entry}"
                    );
                }
            }
            assert_eq!(rejected, 0, "a delivered answer was never rejected");
            let finals = events_of(&events, "final");
            assert_eq!(
                finals
                    .last()
                    .map(|event| event["data"]["text"].as_str().unwrap_or_default().trim()),
                Some(answer.trim()),
                "the recorded final answer is the delivered answer"
            );
        }
        Outcome::Refused(stderr) => {
            // Refused: the model never produced a valid report, so the turn is an
            // error naming the contract, never a silent success.
            assert!(
                stderr.starts_with("Error: Task contract not satisfied: "),
                "unexpected refusal: {stderr}"
            );
            assert_eq!(rejected, 1, "a refused turn records exactly one rejection");
            assert!(
                events_of(&events, "task_report").is_empty(),
                "a refused turn records no task_report"
            );
            assert!(
                events_of(&events, "run_error").iter().any(|event| {
                    event["data"]["message"]
                        .as_str()
                        .is_some_and(|message| message.starts_with("Task contract not satisfied: "))
                }),
                "a refused turn records the contract error; events: {events:?}"
            );
        }
    }

    // `--model-only` keeps its own output contract: no task contract, no
    // report, no violation.
    let (outcome, events) = home.run_turn(
        "Reply with the single word OK and nothing else.",
        &["--model-only"],
    );
    assert!(
        matches!(outcome, Outcome::Delivered(_)),
        "a model-only turn is delivered without a report"
    );
    assert!(events_of(&events, "task_contract").is_empty());
    assert!(events_of(&events, "task_report").is_empty());
    assert!(events_of(&events, "contract_violation").is_empty());
}
