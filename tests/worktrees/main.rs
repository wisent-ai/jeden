//! What this product says about git worktrees, driven through the real
//! `jeden` binary.
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
//! The second one is the reason this file exists. A capability list that
//! nothing implements reads as true to every consumer, and it stayed wrong
//! for as long as nobody compared it against the code underneath.

use std::process::Command;

fn jeden() -> Command {
    Command::new(env!("CARGO_BIN_EXE_jeden"))
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
