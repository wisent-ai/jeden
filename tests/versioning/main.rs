//! The version gate's tools, through their real command line: the versioning
//! rule reproduces every AutoVersion v0.1.0 conformance fixture, the surface
//! extractor reads this checkout's dispatcher and slash catalogue, and the
//! protocol gate passes on the committed schema, golden envelopes and SDKs.
//!
//! `autoversion-fixtures.json` is the fenced block of AutoVersion's
//! FIXTURES.md at tag v0.1.0, copied unchanged: the rule's SPEC makes those
//! cases the contract every port reproduces.

use serde_json::{json, Value};
use std::env;
use std::fs;
use std::path::PathBuf;
use std::process::{Command, Output};
use std::sync::LazyLock;

fn root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

/// The tool, built once per test run into its own target directory so the
/// build does not wait on the lock this `cargo test` holds.
static TOOL: LazyLock<PathBuf> = LazyLock::new(|| {
    let target = root().join("target/tools");
    let cargo = env::var("CARGO").expect("cargo test sets CARGO");
    let built = Command::new(cargo)
        .args([
            "build",
            "--quiet",
            "--locked",
            "--manifest-path",
            "tools/Cargo.toml",
        ])
        .arg("--target-dir")
        .arg(&target)
        .current_dir(root())
        .status()
        .expect("build jeden-tools");
    assert!(built.success(), "jeden-tools did not build");
    target.join(format!("debug/jeden-tools{}", env::consts::EXE_SUFFIX))
});

fn run(arguments: &[&str]) -> Output {
    Command::new(&*TOOL)
        .args(arguments)
        .current_dir(root())
        .output()
        .expect("run jeden-tools")
}

fn fixtures() -> Value {
    let path = root().join("tests/versioning/autoversion-fixtures.json");
    serde_json::from_str(&fs::read_to_string(path).expect("read fixtures")).expect("fixtures JSON")
}

/// Write one case's two surfaces where the tool can read them.
fn surfaces(name: &str, case: &Value) -> (String, String) {
    let directory = root().join("target/versioning-fixtures");
    fs::create_dir_all(&directory).expect("create fixture directory");
    let slug = name.replace(|character: char| !character.is_ascii_alphanumeric(), "-");
    let published = directory.join(format!("{slug}.published.json"));
    let candidate = directory.join(format!("{slug}.candidate.json"));
    fs::write(
        &published,
        json!({"surface": case["published"]}).to_string(),
    )
    .expect("write published");
    fs::write(
        &candidate,
        json!({"surface": case["candidate"]}).to_string(),
    )
    .expect("write candidate");
    (
        published.display().to_string(),
        candidate.display().to_string(),
    )
}

fn decide(name: &str, case: &Value, declared_breaking: bool) -> Output {
    let (published, candidate) = surfaces(name, case);
    let current = case["current"].as_str().expect("current version");
    let mut arguments = vec![
        "versioning",
        "decide",
        "--current",
        current,
        "--published-surface",
        &published,
        "--candidate-surface",
        &candidate,
        "--json",
    ];
    if declared_breaking {
        arguments.push("--breaking");
    }
    run(&arguments)
}

#[test]
fn rule_reproduces_every_classification_fixture() {
    for case in fixtures()["classify"].as_array().expect("classify cases") {
        let name = case["name"].as_str().expect("case name");
        let output = decide(name, case, case["declared_breaking"] == true);
        assert!(
            output.status.success(),
            "{name}: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        let answer: Value = serde_json::from_slice(&output.stdout).expect("decision JSON");
        let expect = &case["expect"];
        assert_eq!(answer["change"], expect["class"], "{name}");
        assert_eq!(answer["next"], expect["next"], "{name}");
        assert_eq!(answer["removed"], expect["removed"], "{name}");
        assert_eq!(answer["added"], expect["added"], "{name}");
    }
}

#[test]
fn rule_refuses_every_refusal_fixture_by_name() {
    for case in fixtures()["refuse"].as_array().expect("refuse cases") {
        let name = case["name"].as_str().expect("case name");
        let output = decide(name, case, false);
        assert!(
            !output.status.success(),
            "{name}: the rule answered instead of refusing"
        );
        let refusal = case["expect"]["refusal"].as_str().expect("refusal name");
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(
            stderr.contains(&format!("refused ({refusal}):")),
            "{name}: {stderr}"
        );
    }
}

#[test]
fn surface_reads_the_dispatcher_and_the_slash_catalogue() {
    let output = run(&["surface"]);
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let document: Value = serde_json::from_slice(&output.stdout).expect("surface JSON");
    let names = document["surface"].as_array().expect("surface list");
    let named = |name: &str| names.iter().any(|entry| entry == name);
    assert!(
        named("cli:run"),
        "the dispatcher's `run` command is missing"
    );
    assert!(
        named("slash:/help"),
        "the builtin `/help` command is missing"
    );
    let mut sorted = names.clone();
    sorted.sort_by(|left, right| left.as_str().cmp(&right.as_str()));
    sorted.dedup();
    assert_eq!(&sorted, names, "the surface is not a sorted set");
}

#[test]
fn surface_refuses_a_tree_without_a_dispatcher() {
    let empty = root().join("target/versioning-empty-tree");
    fs::create_dir_all(&empty).expect("create empty tree");
    let output = run(&["surface", &empty.display().to_string()]);
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("cannot read rust/main.rs"));
}

#[test]
fn protocol_gate_passes_on_the_committed_contract() {
    let output = run(&["protocol-check"]);
    assert!(
        output.status.success(),
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn manifest_version_reads_the_committed_package_version() {
    let output = run(&["versioning", "manifest-version"]);
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let version = String::from_utf8_lossy(&output.stdout).trim().to_string();
    let manifest = fs::read_to_string(root().join("Cargo.toml")).expect("read Cargo.toml");
    assert!(
        manifest.contains(&format!("version = \"{version}\"")),
        "{version}"
    );
}
