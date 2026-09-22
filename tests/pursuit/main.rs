//! Real CLI admission and durable-refusal journeys for `jeden pursue`, with no
//! model stand-ins.
//!
//! They qualify a built candidate, so they need what the fleet's build
//! supplies: the candidate (`JEDEN_TEST_BINARY`, else `$WISENT_OUTPUT_DIR/bin/jeden`,
//! else the binary this `cargo test` built) and `WISENT_SOURCE_COMMIT`, the
//! revision it was built from. A missing source binding fails the journey.
//! The release recipe runs them with
//! `jeden-tools release cargo test --test pursuit -- --ignored`.
//!
//! Each journey keeps its commands, exit statuses, the executable's hash and
//! source identity in `report.json` under `$WISENT_OUTPUT_DIR/pursuit-tests`,
//! or `target/pursuit-runs` on a development host.

use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use std::env;
use std::fs;
use std::path::PathBuf;
use std::process::{Command, Output};

/// The request document layout `jeden pursue --request-file` accepts.
const REQUEST_SCHEMA_VERSION: u64 = 1;

fn root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

fn fresh() -> String {
    uuid::Uuid::new_v4().simple().to_string()
}

struct Journey {
    root: PathBuf,
    binary: PathBuf,
    env: BTreeMap<String, String>,
    report: Value,
    identity: String,
    request: Value,
}

impl Journey {
    fn start(name: &str) -> Self {
        let output = env::var("WISENT_OUTPUT_DIR")
            .ok()
            .filter(|value| !value.is_empty());
        let evidence = match &output {
            Some(output) => PathBuf::from(output).join("pursuit-tests"),
            None => root().join("target/pursuit-runs"),
        };
        let directory = evidence.join(fresh());
        fs::create_dir_all(directory.join("home")).expect("create journey home");
        let directory = fs::canonicalize(&directory).expect("journey directory");
        let binary = match (env::var("JEDEN_TEST_BINARY").ok(), &output) {
            (Some(binary), _) if !binary.is_empty() => PathBuf::from(binary),
            (_, Some(output)) => PathBuf::from(output).join("bin/jeden"),
            _ => PathBuf::from(env!("CARGO_BIN_EXE_jeden")),
        };
        let binary = fs::canonicalize(&binary).unwrap_or_else(|error| {
            panic!(
                "the real Jeden candidate {} is unavailable: {error}",
                binary.display()
            )
        });
        let digest = Sha256::digest(fs::read(&binary).expect("read candidate"));
        let ambient = env::vars().collect::<BTreeMap<_, _>>();
        let mut journey_env = ambient.clone();
        journey_env.remove("JEDEN_LANGUAGE");
        journey_env.insert("HOME".into(), directory.join("home").display().to_string());
        journey_env.insert(
            "JEDEN_SESSION_ROOT".into(),
            directory.join("sessions").display().to_string(),
        );
        journey_env.insert(
            "JEDEN_PURSUIT_STATE_ROOT".into(),
            directory.join("requests").display().to_string(),
        );
        let identity = format!("request-{}", fresh());
        // An existing non-checkout exercises a real repository refusal before
        // inference or any external mutation. It is not a copied source tree.
        let request = json!({
            "schema_version": REQUEST_SCHEMA_VERSION,
            "request_id": identity,
            "initiative_id": identity,
            "objective": "Inspect the declared repository without modifying it.",
            "cwd": directory,
            "evidence_refs": [root().join("tests/pursuit/main.rs")],
            "budget_usd": "1",
            "allow_write": false,
            "allow_command": false,
            "repositories": [format!("wisent-ai/unavailable-{}", fresh())],
        });
        let report = json!({
            "journey": name,
            "state": "failed",
            "commands": [],
            "binary": binary,
            "binary_sha256": format!("{digest:x}"),
            "candidate_source_revision": ambient.get("WISENT_SOURCE_COMMIT"),
        });
        let mut journey = Self {
            root: directory,
            binary,
            env: journey_env,
            report,
            identity,
            request,
        };
        match ambient
            .get("WISENT_SOURCE_DIR")
            .filter(|value| !value.is_empty())
        {
            Some(source) => {
                let source = fs::canonicalize(source).expect("WISENT_SOURCE_DIR");
                assert_eq!(source, fs::canonicalize(root()).expect("repository root"));
                journey.report["source_kind"] = json!("archive");
                journey.report["source_sha256"] = json!(ambient.get("WISENT_SOURCE_SHA256"));
                journey.report["checkout_revision"] =
                    journey.report["candidate_source_revision"].clone();
            }
            None => {
                let revision = journey.command(Command::new("git").args(["rev-parse", "HEAD"]));
                assert!(revision.status.success(), "{}", stderr(&revision));
                journey.report["source_kind"] = json!("checkout");
                journey.report["checkout_revision"] =
                    json!(String::from_utf8_lossy(&revision.stdout).trim());
            }
        }
        let version = journey.cli(&["--version"]);
        assert!(
            version.status.success(),
            "the candidate does not run: {}",
            stderr(&version)
        );
        journey
    }

    fn command(&mut self, command: &mut Command) -> Output {
        let described = format!("{command:?}");
        let output = command.current_dir(root()).output().expect("start command");
        if let Some(commands) = self.report["commands"].as_array_mut() {
            commands.push(json!({
                "argv": described,
                "cwd": root(),
                "exit_status": output.status.code(),
                "stdout": String::from_utf8_lossy(&output.stdout),
                "stderr": String::from_utf8_lossy(&output.stderr),
            }));
        }
        self.retain();
        output
    }

    fn cli(&mut self, arguments: &[&str]) -> Output {
        let mut command = Command::new(&self.binary);
        command.args(arguments).env_clear().envs(&self.env);
        self.command(&mut command)
    }

    fn submit(&mut self) -> Output {
        let document = self.root.join("request.json");
        fs::write(&document, self.request.to_string() + "\n").expect("write request");
        let path = document.display().to_string();
        self.cli(&["pursue", "--request-file", &path, "--json"])
    }

    fn state_dir(&self) -> PathBuf {
        self.root.join("requests").join(&self.identity)
    }

    fn saved(&self) -> BTreeMap<String, Value> {
        let path = self.state_dir().join("state.sqlite3");
        let connection = rusqlite::Connection::open_with_flags(
            &path,
            rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
        )
        .unwrap_or_else(|error| panic!("{}: {error}", path.display()));
        let mut statement = connection
            .prepare("SELECT key, data FROM values_store ORDER BY key")
            .expect("query saved values");
        let rows = statement
            .query_map([], |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
            })
            .expect("read saved values");
        rows.map(|row| {
            let (key, data) = row.expect("saved row");
            (key, serde_json::from_str(&data).expect("saved JSON"))
        })
        .collect()
    }

    fn retain(&self) {
        let text = serde_json::to_string_pretty(&self.report).expect("encode report") + "\n";
        fs::write(self.root.join("report.json"), text).expect("write report");
    }

    /// The build producer supplies provenance; a checkout cannot identify the
    /// source of a different executable found on PATH.
    fn passed(mut self) {
        let candidate = self.report["candidate_source_revision"]
            .as_str()
            .map(str::to_string);
        let full_revision = candidate.as_deref().is_some_and(|revision| {
            revision.len() == 40
                && revision
                    .bytes()
                    .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
        });
        assert!(
            full_revision,
            "the candidate carries no WISENT_SOURCE_COMMIT source binding"
        );
        assert_eq!(
            self.report["candidate_source_revision"], self.report["checkout_revision"],
            "candidate source binding is absent or differs from this journey's revision"
        );
        self.report["state"] = json!("passed");
        self.retain();
        println!("Pursuit journey evidence: {}", self.root.display());
    }
}

fn response(output: &Output) -> Value {
    assert!(
        output.status.success(),
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        stderr(output)
    );
    serde_json::from_slice(&output.stdout).expect("JSON response")
}

fn stderr(output: &Output) -> String {
    String::from_utf8_lossy(&output.stderr).into_owned()
}

#[test]
#[ignore = "qualifies a built candidate bound to WISENT_SOURCE_COMMIT"]
fn authority_refusal_does_not_admit_request() {
    let mut journey = Journey::start("authority_refusal_does_not_admit_request");
    journey.request["allow_write"] = json!(true);
    let refused = journey.submit();
    assert!(!refused.status.success());
    assert!(
        stderr(&refused).contains("authority_required:"),
        "{}",
        stderr(&refused)
    );
    assert!(
        !journey.state_dir().exists(),
        "an unauthorized request must not acquire durable execution state"
    );
    let identity = journey.identity.clone();
    let unknown = journey.cli(&["pursue", "--status", &identity, "--json"]);
    assert!(!unknown.status.success());
    assert!(!journey.state_dir().exists());
    journey.passed();
}

#[test]
#[ignore = "qualifies a built candidate bound to WISENT_SOURCE_COMMIT"]
fn blocked_request_survives_reopen_and_rejects_rebinding() {
    let mut journey = Journey::start("blocked_request_survives_reopen_and_rejects_rebinding");
    let identity = journey.identity.clone();
    let blocked = response(&journey.submit());
    assert_eq!(blocked["state"], "blocked", "{blocked}");
    assert!(blocked["run_id"].is_null());
    assert_eq!(blocked["source_revisions"], json!({}));
    let before = journey.saved();
    assert_eq!(before["request"], journey.request);
    assert_eq!(before["response"]["state"], "blocked");
    assert_eq!(response(&journey.submit()), blocked);
    assert_eq!(journey.saved(), before);
    let status = journey.cli(&["pursue", "--status", &identity, "--json"]);
    assert_eq!(response(&status), blocked);
    journey.request["objective"] = json!("Different work must not replace the retained request.");
    let conflict = journey.submit();
    assert!(!conflict.status.success());
    assert!(
        stderr(&conflict).contains("request_id_conflict:"),
        "{}",
        stderr(&conflict)
    );
    assert_eq!(journey.saved(), before);
    let resumed = response(&journey.cli(&["pursue", "--resume-run", &identity, "--json"]));
    assert_eq!(resumed["state"], "blocked");
    assert_eq!(journey.saved()["request"], before["request"]);
    assert!(
        !journey.state_dir().join("inference.sqlite3").exists(),
        "a non-checkout must refuse before inference"
    );
    journey.report["persisted_request"] = journey.saved()["request"].clone();
    journey.passed();
}
