//! What a workspace allows: no git worktree, and no tool write into the
//! workspace's own Jeden state directory.
//!
//! The operator's rule is "MA BYC NIEMOZLIWE UZYCIE WORKTREES. ZERO
//! SUBAGENTOW NA OSOBNYCH WORKTREES". Two things follow, and both are
//! observable from outside the process, which is why they are tested here
//! rather than asserted against internals:
//!
//! - the command surface offers no way to make one;
//! - the runtime no longer advertises `git-worktree` as a way it isolates a
//!   job workspace, and every strategy it does advertise is one the platform
//!   layer can actually return.
//!
//! That second one is the reason this file exists. A capability list that
//! nothing implements reads as true to every consumer, and it stayed wrong
//! for as long as nobody compared it against the code underneath.
//!
//! The boundary cases below drive the real tool registry against a real
//! directory on this filesystem. They were written after a turn wrote both
//! files of its assignment into `.jeden/` and reported the work done, and
//! after another turn was refused an absolute in-workspace path, guessed a
//! relative form and created a directory nobody asked for.

use jeden::tool_runtime::runtime_ops::{ArtifactSink, CancellationToken, OperationContext};
use jeden::tool_runtime::{execute, ToolRuntime};
use serde_json::json;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

fn jeden() -> Command {
    Command::new(env!("CARGO_BIN_EXE_jeden"))
}

/// One isolated workspace under this checkout's ignored build directory.
fn workspace(area: &str) -> PathBuf {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("target/workspace-runs")
        .join(format!("{area}-{}", std::process::id()));
    let _ = fs::remove_dir_all(&root);
    fs::create_dir_all(&root).expect("create the isolated workspace");
    root
}

fn runtime(cwd: &Path) -> ToolRuntime<'_> {
    ToolRuntime {
        cwd,
        artifact_dir: None,
        operation: OperationContext::new(
            CancellationToken::new(),
            ArtifactSink::new(cwd.join(".artifacts")),
        ),
        allow_write: true,
        allow_command: false,
        interactive: false,
        ask_user: None,
    }
}

/// Every value `WorkspacePlatform::isolate` can return, across both
/// platforms: `platform/unix/workspace.rs` returns the first three,
/// `platform/windows/workspace.rs` returns the last. Anything the product
/// reports that is absent here is a claim with no implementation behind it.
const IMPLEMENTED_STRATEGIES: [&str; 3] = ["apfs-clone", "reflink-copy", "native-copy"];

#[test]
fn reported_isolation_strategies_are_all_implemented() {
    let output = jeden().arg("doctor").output().expect("jeden doctor runs");
    let report = String::from_utf8(output.stdout).expect("doctor prints UTF-8");
    let marker = "\"isolationStrategies\":[";
    let start = report
        .find(marker)
        .unwrap_or_else(|| panic!("doctor reports isolation strategies: {report}"))
        + marker.len();
    let end = start
        + report[start..]
            .find(']')
            .expect("the strategy array is closed");
    let reported: Vec<String> = report[start..end]
        .split(',')
        .map(|value| value.trim().trim_matches('"').to_string())
        .filter(|value| !value.is_empty())
        .collect();

    assert!(
        !reported.is_empty(),
        "doctor reports at least one strategy: {report}"
    );
    assert!(
        !reported.iter().any(|value| value == "git-worktree"),
        "git-worktree is not offered as an isolation strategy: {reported:?}"
    );
    for strategy in &reported {
        assert!(
            IMPLEMENTED_STRATEGIES.contains(&strategy.as_str()),
            "reported strategy {strategy} is one the platform layer returns: {reported:?}"
        );
    }
}

#[test]
fn the_worktree_command_refuses_to_create_one() {
    for attempt in ["add", "create"] {
        let output = jeden()
            .args(["worktree", attempt])
            .output()
            .expect("jeden worktree runs");
        let stderr = String::from_utf8(output.stderr).expect("stderr is UTF-8");
        assert_eq!(
            output.status.code(),
            Some(1),
            "jeden worktree {attempt} exits 1: {stderr}"
        );
        assert_eq!(
            stderr.trim(),
            format!(
                "Error: unexpected argument '{attempt}': usage: jeden worktree [list|clear] [--dry-run] [--json]"
            ),
            "the refusal names the rejected argument and the two supported verbs"
        );
    }
}

#[test]
fn listing_worktrees_still_works_and_claims_no_creation() {
    let output = jeden()
        .args(["worktree", "list"])
        .output()
        .expect("jeden worktree list runs");
    assert_eq!(
        output.status.code(),
        Some(0),
        "listing is the supported read path"
    );
    let text = String::from_utf8(output.stdout).expect("stdout is UTF-8");
    assert!(
        !text.contains("git worktree add"),
        "the listing does not describe a creation path this product lacks: {text}"
    );
}

#[test]
fn no_tool_may_change_the_workspace_state_directory() {
    let root = workspace("state-directory");
    fs::create_dir_all(root.join(".jeden")).expect("create the state directory");
    fs::write(root.join(".jeden/mode-state.json"), "{}").expect("seed the mode state");
    let refusal = execute(
        &runtime(&root),
        "write_file",
        &json!({"path": ".jeden/probe.txt", "content": "PROBE"}),
    )
    .expect_err("a write into the state directory is refused");
    assert!(
        refusal.contains("no tool may change it"),
        "the refusal does not say the state directory is not writable: {refusal}"
    );
    assert!(!root.join(".jeden/probe.txt").exists());
    let read = execute(
        &runtime(&root),
        "read_file",
        &json!({"path": ".jeden/mode-state.json"}),
    )
    .expect("reading the state directory stays allowed");
    assert_eq!(read["content"], "{}");
}

#[test]
fn an_absolute_path_inside_the_workspace_writes_the_file_it_names() {
    let root = workspace("absolute-path");
    let target = root.join("alpha.txt");
    let written = execute(
        &runtime(&root),
        "write_file",
        &json!({"path": target.to_string_lossy(), "content": "ALPHA"}),
    )
    .expect("an absolute path inside the workspace names a file inside it");
    assert_eq!(written["ok"], true);
    assert_eq!(fs::read(&target).expect("the named file"), b"ALPHA");
    assert_eq!(
        fs::read_dir(&root)
            .expect("read the workspace")
            .filter_map(Result::ok)
            .filter(|entry| entry.path().is_dir())
            .count(),
        usize::default(),
        "the write invented a directory the path never named"
    );
    let refusal = execute(
        &runtime(&root),
        "write_file",
        &json!({"path": "/etc/jeden-probe.txt", "content": "ALPHA"}),
    )
    .expect_err("a path outside the workspace is refused");
    assert!(
        refusal.contains("outside this workspace") && refusal.contains(&root.display().to_string()),
        "the refusal does not name the root paths are taken from: {refusal}"
    );
    assert!(!Path::new("/etc/jeden-probe.txt").exists());
}
