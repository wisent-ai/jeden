use super::command::{entry_command, EntryCommand};
use super::*;
use crate::hooks::HookOutcome;
use serde_json::{json, Value};
use std::path::{Path, PathBuf};

/// An exit code that is neither jeden's block signal nor success, so the
/// normalization under test is visible in the result.
const OTHER_FAILURE: i32 = 3;

fn mapped(tama_event: &str) -> (&'static str, String) {
    map_event(tama_event).expect("mapped event")
}

/// Scratch lives under the crate's ignored build directory, the one place this
/// workshop allows throwaway state.
fn scratch(tag: &str) -> PathBuf {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("target/tama-command")
        .join(format!("{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).expect("create scratch");
    root
}

fn resolved(entry: &Value, registry: &Path, catalog: &Value) -> EntryCommand {
    entry_command(entry, registry, catalog).expect("an executable entry")
}

#[test]
fn event_mapping() {
    assert_eq!(
        mapped("user_prompt_submit"),
        ("UserPromptSubmit", String::new())
    );
    assert_eq!(mapped("stop"), ("Stop", String::new()));
    assert_eq!(mapped("session_start"), ("SessionStart", String::new()));
    assert_eq!(
        mapped("session_start:compact"),
        ("SessionStart", String::new())
    );
    assert_eq!(
        mapped("pre_tool_use:bash"),
        ("PreToolUse", "^(run_command|run_process)$".to_string())
    );
    assert_eq!(
        mapped("post_tool_use:bash"),
        ("PostToolUse", "^(run_command|run_process)$".to_string())
    );
    assert!(map_event("unknown_event").is_none());
}

#[test]
fn tool_matchers() {
    assert_eq!(
        tool_matcher("read"),
        "^(read|read_file|read_binary_file|read_archive|read_document)$"
    );
    assert_eq!(tool_matcher("edit"), "^(edit|edit_file|apply_patch)$");
    assert_eq!(tool_matcher("write"), "^(write|write_file)$");
    assert_eq!(tool_matcher("multiedit"), "^(edit|apply_patch)$");
    assert_eq!(tool_matcher("notebook"), "^read_document$");
    assert_eq!(tool_matcher("task"), "^delegate_task$");
    assert_eq!(tool_matcher("todo"), "^todo$");
    assert_eq!(
        tool_matcher("eval"),
        "^(eval_session|python_eval|node_eval)$"
    );
    assert_eq!(tool_matcher("ssh"), "^ssh_exec$");
    assert_eq!(tool_matcher("ask"), "^ask_user$");
    assert_eq!(
        tool_matcher("lookup"),
        "^(search_files|search_text|grep_regex|glob_paths|ast_search)$"
    );
    assert_eq!(tool_matcher("wait"), "^todo$");
    assert_eq!(tool_matcher("goal"), "^todo$");
    assert_eq!(tool_matcher("functions_ask"), "^functions_ask$");
}

#[test]
fn outcome_normalization() {
    let failed = HookOutcome {
        exit_code: OTHER_FAILURE,
        stdout: String::new(),
        stderr: "denied".into(),
    };
    // Blocking events: any failure becomes jeden's block signal.
    assert_eq!(
        normalize_outcome("PreToolUse", true, failed.clone()).exit_code,
        BLOCK_EXIT
    );
    // ... unless the hook explicitly approves.
    let approve = HookOutcome {
        exit_code: OTHER_FAILURE,
        stdout: "{\"decision\":\"approve\"}".into(),
        stderr: String::new(),
    };
    assert_eq!(
        normalize_outcome("PreToolUse", true, approve).exit_code,
        OTHER_FAILURE
    );
    // A block verdict blocks even on a successful exit.
    let verdict = HookOutcome {
        exit_code: PASS_EXIT,
        stdout: "{\"decision\":\"block\",\"reason\":\"no\"}".into(),
        stderr: String::new(),
    };
    assert_eq!(
        normalize_outcome("PreToolUse", true, verdict).exit_code,
        BLOCK_EXIT
    );
    // Non-blocking events never block: the block signal is scrubbed.
    assert_eq!(
        normalize_outcome("PreToolUse", false, failed).exit_code,
        OTHER_FAILURE
    );
    let exit_two = HookOutcome {
        exit_code: BLOCK_EXIT,
        stdout: String::new(),
        stderr: "denied".into(),
    };
    assert_eq!(
        normalize_outcome("PreToolUse", false, exit_two).exit_code,
        PASS_EXIT
    );
    let verdict_nonblocking = normalize_outcome(
        "PreToolUse",
        false,
        HookOutcome {
            exit_code: PASS_EXIT,
            stdout: "{\"decision\":\"block\"}".into(),
            stderr: String::new(),
        },
    );
    assert_eq!(verdict_nonblocking.exit_code, PASS_EXIT);
    assert!(verdict_nonblocking.stdout.is_empty());
}

/// The registration this was written for: `~/.shared-hooks/registry.json`
/// naming `shared-hooks/block_identity_literals.py`, a path that exists beside
/// the registry and nowhere near the workspace a turn runs in.
#[test]
fn a_relative_command_resolves_beside_the_registry() {
    let root = scratch("relative");
    let registry = root.join("registry.json");
    std::fs::write(&registry, "{}").expect("write registry");
    let script = root.join("guard.py");
    std::fs::write(&script, "#!/usr/bin/env python3\n").expect("write guard");

    let entry = json!({"id": "guard", "type": "command", "command": "shared-hooks/guard.py"});
    match resolved(&entry, &registry, &Value::Null) {
        EntryCommand::Runnable(command) => {
            assert_eq!(command, script.to_string_lossy());
        }
        EntryCommand::Missing { tried, .. } => panic!("resolved nothing; tried {tried:?}"),
    }
}

/// The catalog records the file each hook is; when it does, that wins over
/// guessing from the registry's own directory.
#[test]
fn the_catalog_source_names_the_file() {
    let root = scratch("catalog");
    let registry = root.join("registry.json");
    std::fs::write(&registry, "{}").expect("write registry");
    let elsewhere = root.join("runtime");
    std::fs::create_dir_all(&elsewhere).expect("create runtime dir");
    let script = elsewhere.join("guard.py");
    std::fs::write(&script, "#!/usr/bin/env python3\n").expect("write guard");

    let entry = json!({"id": "guard", "type": "command", "command": "shared-hooks/guard.py --json"});
    let catalog = json!({"agentHooks": [{"id": "guard", "source": script.to_string_lossy()}]});
    match resolved(&entry, &registry, &catalog) {
        EntryCommand::Runnable(command) => {
            assert_eq!(command, format!("{} --json", script.to_string_lossy()));
        }
        EntryCommand::Missing { tried, .. } => panic!("resolved nothing; tried {tried:?}"),
    }
}

/// A registration whose file is absent is reported as that, with the paths
/// tried, instead of reaching `sh` and coming back as a verdict on the tool.
#[test]
fn a_registration_with_no_file_says_so() {
    let root = scratch("missing");
    let registry = root.join("registry.json");
    std::fs::write(&registry, "{}").expect("write registry");

    let entry = json!({"id": "gone", "type": "command", "command": "shared-hooks/gone.py"});
    match resolved(&entry, &registry, &Value::Null) {
        EntryCommand::Runnable(command) => panic!("ran something: {command}"),
        EntryCommand::Missing { written, tried } => {
            assert_eq!(written, "shared-hooks/gone.py");
            let reason = super::command::unrunnable_reason("gone", &written, &tried, &registry);
            assert!(reason.contains("TAMA_HOOK_INFRASTRUCTURE"), "{reason}");
            assert!(reason.contains("gone"), "{reason}");
            assert!(reason.contains(&registry.display().to_string()), "{reason}");
        }
    }
}

/// A bare program name is a PATH lookup and stays exactly as registered.
#[test]
fn a_bare_program_is_left_alone() {
    let root = scratch("bare");
    let registry = root.join("registry.json");
    std::fs::write(&registry, "{}").expect("write registry");

    let entry = json!({"id": "cat", "type": "command", "command": "cat -"});
    match resolved(&entry, &registry, &Value::Null) {
        EntryCommand::Runnable(command) => assert_eq!(command, "cat -"),
        EntryCommand::Missing { tried, .. } => panic!("rewrote a PATH program; tried {tried:?}"),
    }
}
