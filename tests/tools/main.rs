//! What a tool path means, driven through the real tool registry against a
//! real directory on this filesystem.
//!
//! Both rules here were written after a real turn lost its assignment to
//! them: one wrote both files its request asked for into the workspace's own
//! `.jeden` state directory and reported the work done, and another was
//! refused an absolute in-workspace path by a sentence that named no
//! alternative, guessed a relative form, and created a directory nobody had
//! asked for.
//!
//! Run: `node tests/contracts/task-contract-lifecycle.probierz.spec.mjs tools`.
//! Runs keep their workspace under this checkout's ignored `target/tool-runs`.

use jeden::tool_runtime::runtime_ops::{ArtifactSink, CancellationToken, OperationContext};
use jeden::tool_runtime::{execute, ToolRuntime};
use serde_json::json;
use std::fs;
use std::path::{Path, PathBuf};

/// One isolated workspace per case, under this checkout's build directory.
fn workspace(area: &str) -> PathBuf {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("target/tool-runs")
        .join(format!("{area}-{}", std::process::id()));
    let _ = fs::remove_dir_all(&root);
    fs::create_dir_all(&root).expect("create the isolated workspace");
    root
}

/// A write-authorized runtime over one workspace, the shape a turn hands to a
/// tool once the operator has granted `--allow-write`.
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
}

#[test]
fn an_absolute_path_outside_the_workspace_is_refused_with_the_root() {
    let root = workspace("outside-path");
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
