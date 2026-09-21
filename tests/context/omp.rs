//! The Omp half of the promise: the same advisor, reachable from a harness
//! Jeden does not own and may not patch.
//!
//! Every case writes into its own isolated directory, never into the
//! operator's `~/.omp/agent/tools`, and drives the real install and report
//! commands rather than inspecting the template.

use crate::fixture::{read_to_string, Workspace};

#[test]
fn installing_writes_a_tool_that_calls_this_binary() {
    let workspace = Workspace::new("omp-install");
    let file = workspace.path("omp-tools/jeden_context.ts");
    let run = workspace.run(&[
        "context",
        "install",
        "--file",
        &file.display().to_string(),
        "--json",
    ]);
    assert!(run.success, "install failed: {}", run.stderr);
    let report = run.json();
    assert_eq!(report["target"], "file");
    assert_eq!(report["tool"], "context_recommend");
    assert_eq!(report["changed"], true);
    let source = read_to_string(&file);
    assert!(
        source.contains("name: \"context_recommend\""),
        "the installed tool must declare the advisor's tool name: {source}"
    );
    assert!(
        source.contains(env!("CARGO_BIN_EXE_jeden")),
        "the installed tool must call the binary that rendered it, not a name on PATH"
    );
    assert!(
        source.contains("\"context\", \"recommend\""),
        "the installed tool must call the product command, not reimplement it"
    );
}

#[test]
fn a_second_install_changes_nothing_and_says_so() {
    let workspace = Workspace::new("omp-idempotent");
    let file = workspace.path("omp-tools/jeden_context.ts");
    let path = file.display().to_string();
    assert!(workspace
        .run(&["context", "install", "--file", &path])
        .success);
    let again = workspace.run(&["context", "install", "--file", &path, "--json"]);
    assert!(again.success, "the second install failed: {}", again.stderr);
    assert_eq!(again.json()["changed"], false);
    let reported = workspace.run(&["context", "installed", "--file", &path, "--json"]);
    assert!(reported.success, "installed failed: {}", reported.stderr);
    assert_eq!(reported.json()["state"], "current");
}

#[test]
fn an_edited_tool_is_reported_stale_and_refused() {
    let workspace = Workspace::new("omp-stale");
    let file = workspace.path("omp-tools/jeden_context.ts");
    let path = file.display().to_string();
    assert!(workspace
        .run(&["context", "install", "--file", &path])
        .success);
    std::fs::write(&file, "export default null;\n").expect("edit the installed tool by hand");
    let reported = workspace.run(&["context", "installed", "--file", &path]);
    assert!(
        !reported.success,
        "a stale tool must exit non-zero: {}",
        reported.stdout
    );
    assert!(
        reported.stderr.contains("stale:") && reported.stderr.contains("jeden context install"),
        "the report must name the repair: {}",
        reported.stderr
    );
    let repaired = workspace.run(&["context", "install", "--file", &path, "--json"]);
    assert_eq!(repaired.json()["changed"], true);
    assert!(read_to_string(&file).contains("context_recommend"));
}

#[test]
fn a_missing_tool_is_reported_absent() {
    let workspace = Workspace::new("omp-absent");
    let path = workspace
        .path("omp-tools/never-installed.ts")
        .display()
        .to_string();
    let reported = workspace.run(&["context", "installed", "--file", &path]);
    assert!(
        !reported.success,
        "an absent tool must exit non-zero: {}",
        reported.stdout
    );
    assert!(
        reported.stderr.contains("absent:"),
        "the report must say the tool is absent: {}",
        reported.stderr
    );
}

#[test]
fn installing_without_a_target_is_refused() {
    let workspace = Workspace::new("omp-no-target");
    let run = workspace.run(&["context", "install"]);
    assert!(!run.success, "a missing target must refuse: {}", run.stdout);
    assert!(
        run.stderr.contains("require --omp or --file"),
        "the refusal must name both targets: {}",
        run.stderr
    );
}
